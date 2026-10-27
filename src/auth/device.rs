use crate::{
    config::{Settings, Store},
    error::auth_required,
};
use anyhow::{Context, Result};
use azure_core::credentials::AccessToken;
use fs2::FileExt;
use oauth2::{
    ClientId, DeviceAuthorizationUrl, RefreshToken, Scope, StandardDeviceAuthorizationResponse,
    TokenResponse, TokenUrl, basic::BasicClient,
};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

#[derive(Deserialize, Serialize)]
struct Session {
    tenant_id: String,
    client_id: String,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

fn expires_at(expires_in: Option<std::time::Duration>) -> Result<i64> {
    let lifetime = expires_in.context("Token expiry missing")?;
    let lifetime = Duration::try_from(lifetime).context("Token expiry too large")?;
    anyhow::ensure!(lifetime.is_positive(), "Token expiry must be positive");
    OffsetDateTime::now_utc()
        .checked_add(lifetime)
        .map(|expiry| expiry.unix_timestamp())
        .context("Token expiry out of range")
}

async fn lock(store: &Store) -> Result<std::fs::File> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let file = store.lock_file()?;
        // A crashed process releases this OS lock. Bound waiting for another refresher.
        for _ in 0..400 {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(file),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("Authentication cache is busy. Retry after the other command finishes.")
    })
    .await?
}

pub async fn silent(store: &Store, settings: &Settings) -> Result<AccessToken> {
    let (tenant, _) = settings.identity()?;
    silent_at(
        store,
        settings,
        &format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
    )
    .await
}

async fn silent_at(store: &Store, settings: &Settings, token_url: &str) -> Result<AccessToken> {
    let (tenant, client_id) = settings.identity()?;
    let _lock = lock(store).await?;
    let mut session: Session = store.read_session()?.ok_or_else(auth_required)?;
    if session.tenant_id != tenant || session.client_id != client_id {
        return Err(auth_required());
    }
    let now = OffsetDateTime::now_utc();
    if session.expires_at <= (now + Duration::minutes(5)).unix_timestamp() {
        let client = BasicClient::new(ClientId::new(client_id))
            .set_token_uri(TokenUrl::new(token_url.into())?);
        let refresh = RefreshToken::new(session.refresh_token.clone());
        let response = client
            .exchange_refresh_token(&refresh)
            .add_scope(Scope::new(super::SCOPE.into()))
            .request_async(&http()?)
            .await
            .map_err(|_| auth_required())?;
        session.access_token = response.access_token().secret().clone();
        if let Some(refresh) = response.refresh_token() {
            session.refresh_token = refresh.secret().clone();
        }
        session.expires_at = expires_at(response.expires_in())?;
        store.write("auth.json", &session)?;
    }
    Ok(AccessToken::new(
        session.access_token,
        OffsetDateTime::from_unix_timestamp(session.expires_at)?,
    ))
}

pub async fn login(
    store: &Store,
    settings: &Settings,
    notify: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
    let (tenant, _) = settings.identity()?;
    login_at(
        store,
        settings,
        notify,
        DeviceAuthorizationUrl::new(format!(
            "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/devicecode"
        ))?,
        TokenUrl::new(format!(
            "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"
        ))?,
    )
    .await
}

async fn login_at(
    store: &Store,
    settings: &Settings,
    notify: tokio::sync::mpsc::UnboundedSender<String>,
    device_url: DeviceAuthorizationUrl,
    token_url: TokenUrl,
) -> Result<()> {
    let (tenant, client_id) = settings.identity()?;
    let client = BasicClient::new(ClientId::new(client_id.clone()))
        .set_token_uri(token_url)
        .set_device_authorization_url(device_url);
    let http = http()?;
    let details: StandardDeviceAuthorizationResponse = client.exchange_device_code()
        .add_scope(Scope::new(super::SCOPE.into())).add_scope(Scope::new("offline_access".into()))
        .request_async(&http).await.map_err(|_| anyhow::anyhow!("Cannot start device login. Check tenant, public-client app registration, consent and tenant policy."))?;
    let _ = notify.send(format!(
        "Open {}\nEnter code {}\nWaiting for sign-in...",
        details.verification_uri().as_str(),
        details.user_code().secret()
    ));
    let response = client
        .exchange_device_access_token(&details)
        .request_async(&http, tokio::time::sleep, None)
        .await
        .map_err(|_| {
            anyhow::anyhow!("Device sign-in failed or expired. Check tenant policy and retry.")
        })?;
    let session = Session {
        tenant_id: tenant,
        client_id,
        access_token: response.access_token().secret().clone(),
        refresh_token: response
            .refresh_token()
            .context("No refresh token returned. Check offline_access consent.")?
            .secret()
            .clone(),
        expires_at: expires_at(response.expires_in())?,
    };
    let _lock = lock(store).await?;
    store.write("auth.json", &session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, method, path},
    };
    fn fixture() -> Result<(tempfile::TempDir, Store, Settings)> {
        let root = tempfile::tempdir()?;
        let store = Store {
            root: root.path().join("config"),
        };
        let settings = Settings {
            tenant_id: Some("11111111-1111-1111-1111-111111111111".into()),
            client_id: Some("22222222-2222-2222-2222-222222222222".into()),
            ..Default::default()
        };
        Ok((root, store, settings))
    }
    fn session(settings: &Settings, expiry: i64) -> Result<Session> {
        let (tenant_id, client_id) = settings.identity()?;
        Ok(Session {
            tenant_id,
            client_id,
            access_token: "old-access-secret".into(),
            refresh_token: "old-refresh-secret".into(),
            expires_at: expiry,
        })
    }
    #[tokio::test]
    async fn missing_session_never_prompts() -> Result<()> {
        let (_root, store, settings) = fixture()?;
        let error = silent_at(&store, &settings, "http://127.0.0.1:1/token")
            .await
            .err()
            .context("Expected missing session error")?;
        assert_eq!(
            error
                .downcast_ref::<crate::error::Failure>()
                .context("Expected auth failure")?
                .code,
            3
        );
        Ok(())
    }
    #[tokio::test]
    async fn valid_cached_token_requires_no_network() -> Result<()> {
        let (_root, store, settings) = fixture()?;
        store.write(
            "auth.json",
            &session(
                &settings,
                (OffsetDateTime::now_utc() + Duration::hours(1)).unix_timestamp(),
            )?,
        )?;
        assert_eq!(
            silent_at(&store, &settings, "http://127.0.0.1:1/token")
                .await?
                .token
                .secret(),
            "old-access-secret"
        );
        Ok(())
    }
    #[tokio::test]
    async fn identity_mismatch_cannot_reuse_a_cached_session() -> Result<()> {
        let (_root, store, mut settings) = fixture()?;
        store.write(
            "auth.json",
            &session(
                &settings,
                (OffsetDateTime::now_utc() + Duration::hours(1)).unix_timestamp(),
            )?,
        )?;
        settings.client_id = Some("33333333-3333-3333-3333-333333333333".into());
        assert!(
            silent_at(&store, &settings, "http://127.0.0.1:1/token")
                .await
                .is_err()
        );
        Ok(())
    }
    #[tokio::test]
    async fn concurrent_refresh_rotates_once_under_file_lock() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = MockServer::start().await;
        let (_root, store, settings) = fixture()?;
        store.write("auth.json", &session(&settings, 0)?)?;
        Mock::given(method("POST")).and(path("/token")).and(body_string_contains("grant_type=refresh_token")).and(body_string_contains("refresh_token=old-refresh-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"access_token":"new-access-secret","refresh_token":"rotated-refresh-secret","token_type":"Bearer","expires_in":3600}))).expect(1).mount(&server).await;
        let url = format!("{}/token", server.uri());
        let (a, b) = tokio::join!(
            silent_at(&store, &settings, &url),
            silent_at(&store, &settings, &url)
        );
        assert_eq!(a?.token.secret(), "new-access-secret");
        assert_eq!(b?.token.secret(), "new-access-secret");
        let cached: Session = store.read_session()?.context("Expected cached session")?;
        assert_eq!(cached.refresh_token, "rotated-refresh-secret");
        let settings_text = serde_json::to_string(&settings)?;
        assert!(!settings_text.contains("secret"));
        Ok(())
    }
    #[tokio::test]
    async fn revoked_refresh_does_not_leak_response_secrets() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = MockServer::start().await;
        let (_root, store, settings) = fixture()?;
        store.write("auth.json", &session(&settings, 0)?)?;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({"error":"invalid_grant","error_description":"old-refresh-secret"}))).mount(&server).await;
        let error = silent_at(&store, &settings, &format!("{}/token", server.uri()))
            .await
            .err()
            .context("Expected revoked-session error")?;
        assert!(!format!("{error:?}").contains("secret"));
        assert_eq!(
            error
                .downcast_ref::<crate::error::Failure>()
                .context("Expected auth failure")?
                .code,
            3
        );
        Ok(())
    }

    async fn device_challenge(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/devicecode"))
            .and(body_string_contains(
                "client_id=22222222-2222-2222-2222-222222222222",
            ))
            .and(body_string_contains(
                "scope=499b84ac-1321-427f-aa17-267ca6975798%2F.default",
            ))
            .and(body_string_contains("offline_access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code":"fixture-device-secret",
                "user_code":"ABCD-EFGH",
                "verification_uri":"https://microsoft.com/devicelogin",
                "expires_in":900,
                "interval":1
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn device_login_polls_and_saves_a_refreshable_session() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = MockServer::start().await;
        let (_root, store, settings) = fixture()?;
        device_challenge(&server).await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({"error":"authorization_pending"})),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("device_code=fixture-device-secret"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token":"device-access-secret",
                "refresh_token":"device-refresh-secret",
                "token_type":"Bearer",
                "expires_in":3600
            })))
            .expect(1)
            .mount(&server)
            .await;
        let (notify, mut messages) = tokio::sync::mpsc::unbounded_channel();
        login_at(
            &store,
            &settings,
            notify,
            DeviceAuthorizationUrl::new(format!("{}/devicecode", server.uri()))?,
            TokenUrl::new(format!("{}/token", server.uri()))?,
        )
        .await?;
        let progress = messages.try_recv()?;
        assert!(progress.contains("ABCD-EFGH"));
        assert!(progress.contains("https://microsoft.com/devicelogin"));
        assert!(!progress.contains("secret"));
        let cached: Session = store.read_session()?.context("Expected cached session")?;
        assert_eq!(cached.access_token, "device-access-secret");
        assert_eq!(cached.refresh_token, "device-refresh-secret");
        assert!(cached.expires_at > OffsetDateTime::now_utc().unix_timestamp() + 3500);
        assert!(!store.root.join("settings.json").exists());
        Ok(())
    }

    #[tokio::test]
    async fn declined_device_login_preserves_the_existing_session() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = MockServer::start().await;
        let (_root, store, settings) = fixture()?;
        store.write("auth.json", &session(&settings, 0)?)?;
        let before = std::fs::read(store.root.join("auth.json"))?;
        device_challenge(&server).await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error":"access_denied", "error_description":"fixture-device-secret"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let (notify, _messages) = tokio::sync::mpsc::unbounded_channel();
        let error = login_at(
            &store,
            &settings,
            notify,
            DeviceAuthorizationUrl::new(format!("{}/devicecode", server.uri()))?,
            TokenUrl::new(format!("{}/token", server.uri()))?,
        )
        .await
        .err()
        .context("Expected declined-login error")?;
        assert!(!format!("{error:?}").contains("secret"));
        assert_eq!(std::fs::read(store.root.join("auth.json"))?, before);
        Ok(())
    }

    #[test]
    fn invalid_token_lifetimes_are_rejected() {
        assert!(expires_at(None).is_err());
        assert!(expires_at(Some(std::time::Duration::ZERO)).is_err());
        assert!(expires_at(Some(std::time::Duration::MAX)).is_err());
    }
}
