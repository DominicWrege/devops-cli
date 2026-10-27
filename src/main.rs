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
mod output;
mod pipeline;
mod service;
mod tui;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command, PipelineCommand};
use config::Store;
use std::io::Write;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
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

fn emit(text: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", output::clean(text))?;
    out.flush()?;
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    let store = Store::home()?;
    let mut settings = store.load()?;
    match &cli.command {
        Command::Tui => return tui::run(store, settings, cli.org, cli.project).await,
        Command::Config => {
            return emit(&format!(
                "Organization: {}\nProject: {}\nTenant ID: {}\nClient ID: {}",
                settings.organization.as_deref().unwrap_or(""),
                settings.project.as_deref().unwrap_or(""),
                settings.tenant_id.as_deref().unwrap_or(""),
                settings.client_id.as_deref().unwrap_or("")
            ));
        }
        Command::Init {
            tenant_id,
            client_id,
        } => {
            if let Some(org) = &cli.org {
                settings.organization = Some(config::organization(org)?);
                settings.project_id = None;
            }
            if let Some(project) = &cli.project {
                settings.project = Some(project.clone());
                settings.project_id = None;
            }
            if let Some(tenant) = tenant_id {
                settings.tenant_id = Some(tenant.clone());
            }
            if let Some(client) = client_id {
                settings.client_id = Some(client.clone());
            }
            settings.target(None, None)?;
            settings.identity()?;
            store.save(&settings)?;
            return emit("Saved defaults. Open `devops tui` to sign in.");
        }
        Command::Pr { command } => command.validate()?,
        Command::Pipeline { command } => command.validate()?,
        Command::Repo { command } => command.validate()?,
        _ => (),
    }
    let target = if matches!(cli.command, Command::Auth { .. }) {
        None
    } else {
        Some(settings.target(cli.org.as_deref(), cli.project.as_deref())?)
    };
    let credential = auth::Credential::new(store, settings.clone())?;
    credential.token().await?; // Authentication failures have a stable non-interactive exit code.
    if matches!(cli.command, Command::Auth { .. }) {
        return emit("Signed in. Silent token acquisition succeeded.");
    }
    let api = service::Service::new(credential, target.context("Remote command has no target")?)?;
    let result = match cli.command {
        Command::Pr { command } => api.pr_action(&command).await?,
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

async fn watch(api: &service::Service, run: i32) -> Result<()> {
    let mut previous = String::new();
    loop {
        let build = api.build_run(run).await?;
        let summary = service::build_summary(&build);
        if summary != previous {
            emit(&summary)?;
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
