//! Azure DevOps' browser console feed. REST logs remain the authoritative archive.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

pub enum Event {
    Status(String),
    Lines { record: String, lines: Vec<String> },
}

pub struct Client {
    credential: azure_devops_rust_api::Credential,
    organization: String,
}

impl Client {
    pub fn new(credential: azure_devops_rust_api::Credential, organization: String) -> Self {
        Self {
            credential,
            organization,
        }
    }

    async fn authorization(&self) -> Result<String> {
        use azure_devops_rust_api::Credential;
        match &self.credential {
            Credential::Pat(pat) => Ok(format!("Basic {}", STANDARD.encode(format!(":{pat}")))),
            Credential::TokenCredential(credential) => {
                let token = credential
                    .get_token(&[crate::auth::SCOPE], None)
                    .await
                    .map_err(|_| anyhow::anyhow!("Live console authentication failed"))?;
                Ok(format!("Bearer {}", token.token.secret()))
            }
            Credential::Unauthenticated => anyhow::bail!("Live console requires sign-in"),
        }
    }

    pub async fn run(self, project: String, run: i32, tx: mpsc::Sender<Event>) {
        loop {
            if tx
                .send(Event::Status("Connecting to live console".into()))
                .await
                .is_err()
            {
                return;
            }
            let result = self.connect(&project, run, &tx).await;
            let message = match result {
                Ok(()) => "Live console disconnected".into(),
                Err(error) => error.to_string(),
            };
            if tx
                .send(Event::Status(format!(
                    "{message}. Uploaded logs still refresh every 2s; reconnecting in 15s."
                )))
                .await
                .is_err()
            {
                return;
            }
            tokio::select! {
                _ = tx.closed() => return,
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            }
        }
    }

    async fn connect(&self, project: &str, run: i32, tx: &mpsc::Sender<Event>) -> Result<()> {
        // Never include request URLs, connection tokens, headers or response bodies in errors.
        let authorization = self.authorization().await?;
        let mut header = reqwest::header::HeaderValue::from_str(&authorization)
            .map_err(|_| anyhow::anyhow!("Invalid live console credential"))?;
        header.set_sensitive(true);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::AUTHORIZATION, header.clone());
        headers.insert(
            "x-tfs-fedauthredirect",
            reqwest::header::HeaderValue::from_static("Suppress"),
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()?;
        let base = format!(
            "https://dev.azure.com/{}",
            crate::service::segment(&self.organization)
        );
        let context = read_json(
            http.get(format!("{base}/_apis/connectionData"))
                .query(&[("connectOptions", "1")]),
            "Live console discovery",
        )
        .await?;
        let context = context["instanceId"]
            .as_str()
            .context("Live console account ID missing")?;
        let route = format!("{base}/_apis/{}/signalr", crate::service::segment(project));
        let hubs = r#"[{"name":"builddetailhub"}]"#;
        let parameters = [
            ("transport", "webSockets"),
            ("contextToken", context),
            ("clientProtocol", "1.5"),
            ("connectionData", hubs),
        ];
        let negotiation = read_json(
            http.get(format!("{route}/negotiate")).query(&parameters),
            "Live console negotiation",
        )
        .await?;
        let token = negotiation["ConnectionToken"]
            .as_str()
            .context("Live console connection token missing")?;
        let mut url = reqwest::Url::parse(&format!(
            "wss://dev.azure.com/_signalr/{}/_apis/{}/signalr/connect",
            crate::service::segment(&self.organization),
            crate::service::segment(project)
        ))?;
        url.query_pairs_mut()
            .extend_pairs(parameters)
            .append_pair("connectionToken", token);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| anyhow::anyhow!("Invalid live console address"))?;
        request.headers_mut().insert("authorization", header);
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(2 * 1024 * 1024))
            .max_frame_size(Some(2 * 1024 * 1024));
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(20),
            tokio_tungstenite::connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Live console connection timed out"))?
        .map_err(|_| anyhow::anyhow!("Live console connection failed"))?;
        receive(socket, project, run, tx).await
    }
}

async fn receive<S>(
    mut socket: tokio_tungstenite::WebSocketStream<S>,
    project: &str,
    run: i32,
    tx: &mpsc::Sender<Event>,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(
            json!({"H":"builddetailhub","M":"WatchBuild","A":[project,run],"I":0})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| anyhow::anyhow!("Live console subscription failed"))?;
    let mut subscribed = false;
    loop {
        let message = tokio::select! {
            _ = tx.closed() => return Ok(()),
            message = tokio::time::timeout(Duration::from_secs(60), socket.next()) => message
                .map_err(|_| anyhow::anyhow!("Live console heartbeat timed out"))?
        };
        match message {
            Some(Ok(Message::Text(data))) => {
                let value: Value = serde_json::from_str(&data)
                    .map_err(|_| anyhow::anyhow!("Invalid live console message"))?;
                ensure!(
                    value.get("E").is_none(),
                    "Live console subscription rejected"
                );
                if value.get("I").is_some() && !subscribed {
                    subscribed = true;
                    let _ = tx
                        .send(Event::Status("Live console connected".into()))
                        .await;
                }
                for event in parse_events(&value, run) {
                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Some(Ok(Message::Ping(data))) => {
                socket
                    .send(Message::Pong(data))
                    .await
                    .map_err(|_| anyhow::anyhow!("Live console heartbeat failed"))?;
            }
            Some(Ok(Message::Close(_))) | None => return Ok(()),
            Some(Err(_)) => anyhow::bail!("Live console connection interrupted"),
            _ => {}
        }
    }
}

async fn read_json(request: reqwest::RequestBuilder, operation: &str) -> Result<Value> {
    let response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("{operation} failed"))?;
    ensure!(
        response.status().is_success(),
        "{operation} failed (HTTP {})",
        response.status().as_u16()
    );
    response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("{operation} returned an invalid response"))
}

fn parse_events(value: &Value, run: i32) -> Vec<Event> {
    let mut events = Vec::new();
    if let Some(messages) = value["M"].as_array() {
        for message in messages {
            if !message["H"]
                .as_str()
                .is_some_and(|hub| hub.eq_ignore_ascii_case("builddetailhub"))
            {
                continue;
            }
            if let Some(args) = message["A"].as_array() {
                for arg in args {
                    if arg["buildId"].as_i64() != Some(i64::from(run)) {
                        continue;
                    }
                    let Some(record) = arg["stepRecordId"].as_str().filter(|s| !s.is_empty())
                    else {
                        continue;
                    };
                    let Some(lines) = arg["lines"].as_array() else {
                        continue;
                    };
                    events.push(Event::Lines {
                        record: record.to_owned(),
                        lines: lines
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect(),
                    });
                }
            }
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn websocket_delivers_console_lines_before_any_uploaded_log() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let mut socket = tokio_tungstenite::accept_async(stream).await?;
            let request = socket.next().await.context("Subscription missing")??;
            let request: Value = serde_json::from_str(request.to_text()?)?;
            assert_eq!(request["M"], "WatchBuild");
            assert_eq!(request["A"], json!(["project", 42]));
            socket
                .send(Message::Text(json!({"I":"0"}).to_string().into()))
                .await?;
            socket.send(Message::Text(json!({"M":[{"H":"builddetailhub","M":"logLines","A":[{"buildId":42,"stepRecordId":"task","lines":["still running"]}]}]}).to_string().into())).await?;
            socket.close(None).await?;
            Ok::<(), anyhow::Error>(())
        });
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}")).await?;
        let (tx, mut rx) = mpsc::channel(8);
        tokio::time::timeout(Duration::from_secs(5), receive(socket, "project", 42, &tx)).await??;
        assert!(
            matches!(rx.try_recv()?, Event::Status(status) if status == "Live console connected")
        );
        assert!(
            matches!(rx.try_recv()?, Event::Lines {record, lines} if record == "task" && lines == ["still running"])
        );
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn negotiation_errors_never_expose_credentials_or_response_body() -> Result<()> {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(401).set_body_string("private-response-secret"),
            )
            .mount(&server)
            .await;
        let request =
            reqwest::Client::new().get(format!("{}/?connectionToken=private-token", server.uri()));
        let error = read_json(request, "Live console negotiation")
            .await
            .err()
            .context("Expected failure")?;
        assert_eq!(
            error.to_string(),
            "Live console negotiation failed (HTTP 401)"
        );
        Ok(())
    }

    // Opt-in read-only tenant check. Prints counts/status only, never log contents.
    #[tokio::test]
    #[ignore = "requires saved credentials and DEVOPS_LIVE_TEST_RUN"]
    async fn live_console_smoke() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let store = crate::config::Store::home()?;
        let settings = store.load()?;
        let target = settings.target(None, None)?;
        let credential = crate::auth::Credential::new(store, settings)?;
        let api = crate::service::Service::new(credential, target)?;
        let run: i32 = std::env::var("DEVOPS_LIVE_TEST_RUN")?.parse()?;
        let build = api.build_run(run).await?;
        let project = build["project"]["id"]
            .as_str()
            .context("Project missing")?
            .to_string();
        let (tx, mut rx) = mpsc::channel(64);
        let task = tokio::spawn(api.live().run(project, run, tx));
        let result = tokio::time::timeout(Duration::from_secs(25), async {
            let mut batches = 0;
            while let Some(event) = rx.recv().await {
                match event {
                    Event::Status(status) => eprintln!("{status}"),
                    Event::Lines { lines, .. } => {
                        batches += 1;
                        eprintln!("Console batch received: {} lines", lines.len());
                        if batches >= 2 {
                            return Ok::<(), anyhow::Error>(());
                        }
                    }
                }
            }
            anyhow::bail!("Console channel closed")
        })
        .await;
        task.abort();
        result.context("No two live console batches received within 25 seconds")??;
        Ok(())
    }

    #[test]
    fn console_events_stay_with_their_run_and_task() {
        let value = json!({"M":[{"H":"builddetailhub","M":"logLines","A":[
            {"buildId":42,"stepRecordId":"task-a","lines":["one","two"]},
            {"buildId":99,"stepRecordId":"task-b","lines":["other run"]},
            {"buildId":42,"lines":["no task"]}
        ]}]});
        let events = parse_events(&value, 42);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], Event::Lines { record, lines } if record == "task-a" && lines == &["one", "two"])
        );
        assert!(parse_events(&json!({"M":[]}), 42).is_empty());
    }
}
