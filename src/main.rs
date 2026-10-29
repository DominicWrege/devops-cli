#![deny(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::undocumented_unsafe_blocks
)]
#![deny(
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

mod auth;
mod cli;
mod config;
mod error;
mod live;
mod output;
mod pipeline;
mod secret_prompt;
mod service;
mod tui;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command, PipelineCommand};
use config::{AuthMethod, Store};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
static JSON_OUTPUT: AtomicBool = AtomicBool::new(false);

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    JSON_OUTPUT.store(cli.json, Ordering::Relaxed);
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
            {
                return std::process::ExitCode::SUCCESS;
            }
            let _ = writeln!(
                std::io::stderr().lock(),
                "error: {}",
                output::clean(&error.to_string())
            );
            std::process::ExitCode::from(
                error.downcast_ref::<error::Failure>().map_or(1, |e| e.code),
            )
        }
    }
}

fn emit_json(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, value)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

fn emit(text: &str) -> Result<()> {
    if JSON_OUTPUT.load(Ordering::Relaxed) {
        return emit_json(&serde_json::json!({"message": text}));
    }
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", output::clean(text))?;
    out.flush()?;
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    if cli.json && matches!(cli.command, Command::Tui) {
        return Err(error::invalid(
            "--json is available for CLI commands, not the TUI",
        ));
    }
    let store = Store::home()?;
    let mut settings = store.load()?;
    match &cli.command {
        Command::Login {
            tenant_id,
            client_id,
            pat,
            entra,
        } => {
            return login(
                store,
                settings,
                cli.org.as_deref(),
                cli.project.as_deref(),
                tenant_id.as_deref(),
                client_id.as_deref(),
                if !pat && (*entra || tenant_id.is_some() || client_id.is_some()) {
                    AuthMethod::Entra
                } else {
                    AuthMethod::Pat
                },
            )
            .await;
        }
        Command::Tui => return tui::run(store, settings, cli.org, cli.project).await,
        Command::Config => {
            if cli.json {
                return emit_json(&serde_json::to_value(&settings)?);
            }
            return emit(&format!(
                "Organization: {}\nProject: {}\nTenant ID: {}\nClient ID: {}\nAuthentication: {}",
                settings.organization.as_deref().unwrap_or(""),
                settings.project.as_deref().unwrap_or(""),
                settings.tenant_id.as_deref().unwrap_or(""),
                settings.client_id.as_deref().unwrap_or(""),
                if auth::uses_pat(&settings) {
                    "PAT"
                } else {
                    "Entra"
                }
            ));
        }
        Command::Init {
            tenant_id,
            client_id,
            pat,
            entra,
        } => {
            if let Some(org) = &cli.org {
                settings.organization = Some(config::organization(org)?);
                settings.project_id = None;
            }
            if let Some(project) = &cli.project {
                settings.project = Some(config::project_name(project)?);
                settings.project_id = None;
            }
            if let Some(tenant) = tenant_id {
                settings.tenant_id = Some(tenant.clone());
            }
            if let Some(client) = client_id {
                settings.client_id = Some(client.clone());
            }
            if *entra || tenant_id.is_some() || client_id.is_some() {
                settings.auth = AuthMethod::Entra;
            }
            if *pat || auth::uses_pat(&settings) {
                settings.auth = AuthMethod::Pat;
            }
            settings.target(None, None)?;
            if settings.auth == AuthMethod::Entra && settings.client_id.is_none() {
                return Err(error::invalid(
                    "Missing public-client application ID. Pass `--client-id`.",
                ));
            }
            if settings.auth == AuthMethod::Entra && settings.tenant_id.is_some() {
                settings.identity()?;
            }
            store.save(&settings)?;
            return emit("Saved defaults. Run `devops login` to sign in.");
        }
        Command::Pr { command } => command.validate()?,
        Command::Pipeline { command } => command.validate()?,
        Command::Repo { command } => command.validate()?,
    }
    let target = settings.target(cli.org.as_deref(), cli.project.as_deref())?;
    // Saved PATs belong to organizations, including when --org overrides the default.
    let mut identity = settings.clone();
    identity.organization = Some(target.organization.clone());
    let credential = auth::Credential::new(store, identity)?;
    credential.authenticate().await?; // Authentication failures have a stable non-interactive exit code.
    let api = service::Service::new(credential, target)?;
    if cli.json {
        let value = match &cli.command {
            Command::Pr { command } => api.pr_json(command).await?,
            Command::Repo {
                command: cli::RepoCommand::List,
            } => serde_json::json!(api.repo_list().await?),
            Command::Pipeline {
                command: PipelineCommand::Watch { run },
            } => return watch(&api, *run).await,
            Command::Pipeline { command } => api.pipeline_json(command).await?,
            Command::Repo {
                command: cli::RepoCommand::Create { name },
            } => api.repo_create(name).await?,
            _ => anyhow::bail!("Local command was not handled"),
        };
        return emit_json(&value);
    }
    let result = match cli.command {
        Command::Pr { command } => api.pr_action(&command).await?,
        Command::Repo {
            command: cli::RepoCommand::List,
        } => output::table(
            &api.repo_list().await?,
            &[("ID", "id"), ("NAME", "name"), ("URL", "remoteUrl")],
        )?,
        Command::Pipeline {
            command: PipelineCommand::Watch { run },
        } => return watch(&api, run).await,
        Command::Pipeline { command } => api.pipeline_action(&command).await?,
        Command::Repo {
            command: cli::RepoCommand::Create { name },
        } => {
            let repo = api.repo_create(&name).await?;
            format!(
                "Created repository {}\n{}",
                output::text(&repo, "name"),
                output::text(&repo, "remoteUrl")
            )
        }
        _ => anyhow::bail!("Local command was not handled"),
    };
    emit(&result)
}

async fn login(
    store: Store,
    mut settings: config::Settings,
    org: Option<&str>,
    project: Option<&str>,
    tenant_id: Option<&str>,
    client_id: Option<&str>,
    method: AuthMethod,
) -> Result<()> {
    if let Some(org) = org {
        settings.organization = Some(config::organization(org)?);
        settings.project_id = None;
    }
    if let Some(project) = project {
        settings.project = Some(config::project_name(project)?);
        settings.project_id = None;
    }
    if let Some(tenant) = tenant_id {
        settings.tenant_id = Some(tenant.into());
    }
    if let Some(client) = client_id {
        settings.client_id = Some(client.into());
    }
    settings.auth = method;
    if auth::uses_pat(&settings) {
        settings.auth = AuthMethod::Pat;
    }
    if settings.organization.is_none() {
        let value = prompt("Organization (name or https://dev.azure.com/NAME)", "--org")?;
        settings.organization = Some(config::organization(&value)?);
        settings.project_id = None;
    }
    if settings.auth == AuthMethod::Pat {
        let environment = auth::pat::environment()?;
        let persist = environment.is_none();
        let token = match environment {
            Some(token) => token,
            None => prompt_pat()?,
        };
        let credential = auth::Credential::from_pat(token.clone())?;
        let target = config::Target {
            organization: settings
                .organization
                .clone()
                .context("Login requires an organization")?,
            project: String::new(),
        };
        service::Service::new(credential, target)?
            .check_access()
            .await?;
        if persist {
            auth::pat::save(
                &store,
                settings
                    .organization
                    .as_deref()
                    .context("Missing organization")?,
                &token,
            )?;
        }
        store.save(&settings)?;
        return emit(if persist {
            "Signed in with PAT. Saved for subsequent commands."
        } else {
            "Signed in with PAT from DEVOPS_PAT. The environment override was not saved."
        });
    }
    if settings.tenant_id.is_none() {
        let org = settings
            .organization
            .as_deref()
            .context("Login requires an organization")?;
        settings.tenant_id = Some(auth::discover_tenant(org).await?);
    }
    if settings.client_id.is_none() {
        settings.client_id = Some(prompt("Entra public-client app ID", "--client-id")?);
    }
    let (tenant, client) = settings.identity()?;
    settings.tenant_id = Some(tenant);
    settings.client_id = Some(client);
    store.save(&settings)?;
    let (notify, mut messages) = tokio::sync::mpsc::unbounded_channel::<String>();
    let printer = tokio::spawn(async move {
        while let Some(message) = messages.recv().await {
            if JSON_OUTPUT.load(Ordering::Relaxed) {
                eprintln!("{}", output::clean(&message));
            } else {
                emit(&message)?;
            }
        }
        Ok::<_, anyhow::Error>(())
    });
    let token = auth::login(&store, &settings, notify).await;
    let printed = printer.await?;
    let token = token?;
    printed?;
    emit(&auth::summary(&settings, &token)?)
}

fn prompt_pat() -> Result<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(auth::pat::required());
    }
    let token = secret_prompt::read("Azure DevOps personal access token: ")
        .map_err(|_| anyhow::anyhow!("Cannot read personal access token from the terminal"))?;
    auth::pat::validate(&token)?;
    Ok(token)
}

fn prompt(label: &str, flag: &str) -> Result<String> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Err(error::invalid(format!(
            "Missing {label}. Pass {flag} when input is not a terminal."
        )));
    }
    eprint!("{label}: ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let value = line.trim().to_string();
    if value.is_empty() {
        return Err(error::invalid(format!("{label} cannot be empty")));
    }
    Ok(value)
}

async fn watch(api: &service::Service, run: i32) -> Result<()> {
    let mut previous = String::new();
    loop {
        let build = api.build_run(run).await?;
        let summary = service::build_summary(&build);
        if summary != previous {
            if JSON_OUTPUT.load(Ordering::Relaxed) {
                emit_json(&build)?;
            } else {
                emit(&summary)?;
            }
            previous = summary;
        }
        if build["status"] == "completed" {
            if build["result"] != "succeeded" {
                return Err(error::Failure {
                    code: 5,
                    message: format!(
                        "Run {run} completed with result {}",
                        output::text(&build, "result")
                    ),
                }
                .into());
            }
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}
