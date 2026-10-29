use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "devops", version, about = "Azure DevOps in your terminal.")]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "Organization for this command; saved by login"
    )]
    pub org: Option<String>,
    #[arg(long, global = true, help = "Project for this command; saved by login")]
    pub project: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Print JSON instead of human-readable output"
    )]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Sign in and save the organization and client settings
    Login {
        #[arg(
            long,
            conflicts_with = "pat",
            help = "Override the saved tenant ID; discovered from --org"
        )]
        tenant_id: Option<String>,
        #[arg(
            long,
            conflicts_with = "pat",
            help = "Entra public-client application ID"
        )]
        client_id: Option<String>,
        #[arg(
            long,
            conflicts_with = "entra",
            help = "Use personal access token authentication (default); prompt with hidden input"
        )]
        pat: bool,
        #[arg(
            long,
            conflicts_with = "pat",
            help = "Use Microsoft Entra sign-in instead of a PAT"
        )]
        entra: bool,
    },
    /// Launch the interactive interface and setup wizard
    Tui,
    /// Save explicit defaults without prompting or signing in
    Init {
        #[arg(long, conflicts_with = "pat")]
        tenant_id: Option<String>,
        #[arg(long, conflicts_with = "pat")]
        client_id: Option<String>,
        #[arg(
            long,
            conflicts_with = "entra",
            help = "Save PAT authentication defaults (default for new settings)"
        )]
        pat: bool,
        #[arg(
            long,
            conflicts_with = "pat",
            help = "Save Microsoft Entra authentication defaults"
        )]
        entra: bool,
    },
    /// Inspect saved defaults without displaying credentials
    Config,
    /// Pull requests
    Pr {
        #[command(subcommand)]
        command: PrCommand,
    },
    /// YAML pipelines and runs
    Pipeline {
        #[command(subcommand)]
        command: PipelineCommand,
    },
    /// Remote Git repositories
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
}

#[derive(Subcommand, Clone)]
pub enum RepoCommand {
    /// List repositories in the project
    List,
    /// Create an empty remote repository; never modify local Git state
    Create { name: String },
}

impl RepoCommand {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Create { name } => repository(name),
            Self::List => Ok(()),
        }
    }
}

#[derive(Subcommand, Clone)]
pub enum PrCommand {
    List {
        #[arg(long, value_enum, default_value = "active")]
        state: PrState,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Active PRs assigned to you whose review needs attention
    Status,
    View {
        id: i32,
    },
    Create(PrCreate),
    Edit(PrEdit),
    Comment {
        id: i32,
        #[arg(long)]
        body: String,
    },
    Review {
        id: i32,
        #[arg(long, value_enum)]
        vote: Vote,
    },
    Merge {
        id: i32,
        #[arg(long, value_enum)]
        strategy: MergeStrategy,
        #[arg(long, help = "Acknowledge completion of this PR")]
        yes: bool,
    },
    Abandon {
        id: i32,
        #[arg(long)]
        yes: bool,
    },
    Reactivate {
        id: i32,
    },
}

#[derive(Args, Clone)]
pub struct PrCreate {
    #[arg(long)]
    pub repo: String,
    #[arg(long)]
    pub source: String,
    #[arg(long)]
    pub target: String,
    #[arg(long)]
    pub title: String,
    #[arg(long, default_value = "")]
    pub description: String,
    #[arg(long)]
    pub draft: bool,
    #[arg(long, help = "Azure DevOps identity UUID; repeat to add reviewers")]
    pub reviewer: Vec<String>,
}

#[derive(Args, Clone)]
pub struct PrEdit {
    pub id: i32,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(bool), action = clap::ArgAction::Set)]
    pub draft: Option<bool>,
    #[arg(long)]
    pub add_reviewer: Vec<String>,
    #[arg(long)]
    pub remove_reviewer: Vec<String>,
}

#[derive(ValueEnum, Clone, Copy)]
pub enum PrState {
    Active,
    Completed,
    Abandoned,
    All,
}
impl PrState {
    pub fn api(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
            Self::All => "all",
        }
    }
}
#[derive(ValueEnum, Clone, Copy)]
pub enum Vote {
    Approve,
    ApproveWithSuggestions,
    Reset,
    Wait,
    Reject,
}
impl Vote {
    pub fn number(self) -> i64 {
        match self {
            Self::Approve => 10,
            Self::ApproveWithSuggestions => 5,
            Self::Reset => 0,
            Self::Wait => -5,
            Self::Reject => -10,
        }
    }
}
#[derive(ValueEnum, Clone, Copy)]
pub enum MergeStrategy {
    NoFastForward,
    Squash,
    Rebase,
    RebaseMerge,
}
impl MergeStrategy {
    pub fn api(self) -> &'static str {
        match self {
            Self::NoFastForward => "noFastForward",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
            Self::RebaseMerge => "rebaseMerge",
        }
    }
}

#[derive(Subcommand, Clone)]
pub enum PipelineCommand {
    /// Rename a pipeline
    Rename {
        id: i32,
        name: String,
    },
    /// Move a pipeline to a folder
    Move {
        id: i32,
        folder: String,
    },
    List,
    View {
        id: i32,
    },
    Runs {
        id: i32,
    },
    Run {
        id: i32,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, help = "NAME=VALUE; repeat; values may be typed literals")]
        param: Vec<String>,
        #[arg(long, help = "Non-secret NAME=VALUE override; repeat")]
        variable: Vec<String>,
    },
    Inspect {
        run: i32,
    },
    Logs {
        run: i32,
        #[arg(long)]
        log: i32,
        #[arg(long, default_value_t = 0)]
        start: u64,
    },
    Watch {
        run: i32,
    },
    /// Start a new run from the prior run's pinned repository versions and non-secret inputs
    Rerun {
        id: i32,
        run: i32,
    },
    /// Request a server-side retry of failed work; no job-level selection
    Retry {
        run: i32,
    },
    Cancel {
        run: i32,
        #[arg(long)]
        yes: bool,
    },
    /// Register a pipeline referencing existing committed YAML
    Register {
        #[arg(long)]
        name: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        yaml: String,
    },
}

pub fn acknowledge(yes: bool) -> anyhow::Result<()> {
    if !yes {
        return Err(crate::error::invalid(
            "This operation requires --yes. The CLI never prompts.",
        ));
    }
    Ok(())
}

impl PrCommand {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Merge { yes, .. } | Self::Abandon { yes, .. } => acknowledge(*yes)?,
            Self::Edit(edit)
                if edit.title.is_none()
                    && edit.description.is_none()
                    && edit.draft.is_none()
                    && edit.add_reviewer.is_empty()
                    && edit.remove_reviewer.is_empty() =>
            {
                return Err(crate::error::invalid("No PR edits supplied"));
            }
            _ => (),
        }
        match self {
            Self::View { id }
            | Self::Comment { id, .. }
            | Self::Review { id, .. }
            | Self::Merge { id, .. }
            | Self::Abandon { id, .. }
            | Self::Reactivate { id } => positive(*id)?,
            Self::Edit(edit) => {
                positive(edit.id)?;
                if let Some(title) = &edit.title {
                    nonempty(title, "Title")?;
                }
                for id in edit.add_reviewer.iter().chain(&edit.remove_reviewer) {
                    identity(id)?;
                }
                if edit
                    .add_reviewer
                    .iter()
                    .any(|id| edit.remove_reviewer.contains(id))
                {
                    return Err(crate::error::invalid(
                        "Cannot add and remove the same reviewer in one edit",
                    ));
                }
            }
            Self::Create(args) => {
                repository(&args.repo)?;
                for (value, name) in [
                    (&args.repo, "Repository"),
                    (&args.source, "Source branch"),
                    (&args.target, "Target branch"),
                    (&args.title, "Title"),
                ] {
                    nonempty(value, name)?;
                }
                for id in &args.reviewer {
                    identity(id)?;
                }
            }
            Self::List {
                reviewer, author, ..
            } => {
                for id in reviewer.iter().chain(author) {
                    identity(id)?;
                }
            }
            _ => (),
        }
        if let Self::Comment { body, .. } = self {
            nonempty(body, "Comment")?;
        }
        Ok(())
    }
}

pub(crate) fn positive(id: i32) -> anyhow::Result<()> {
    if id <= 0 {
        return Err(crate::error::invalid("ID must be positive"));
    }
    Ok(())
}
fn nonempty(value: &str, name: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        return Err(crate::error::invalid(format!("{name} cannot be empty")));
    }
    Ok(())
}
fn identity(id: &str) -> anyhow::Result<()> {
    uuid::Uuid::parse_str(id).map_err(|_| {
        crate::error::invalid("Reviewer/author must be an Azure DevOps identity UUID")
    })?;
    Ok(())
}
pub(crate) fn repository(value: &str) -> anyhow::Result<()> {
    nonempty(value, "Repository")?;
    if matches!(value, "." | "..")
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(crate::error::invalid(
            "Repository must be a name or UUID, not a path",
        ));
    }
    Ok(())
}
impl PipelineCommand {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Rename { id, name } => {
                positive(*id)?;
                repository(name)?;
            }
            Self::Move { id, folder } => {
                positive(*id)?;
                nonempty(folder, "Folder")?;
                if folder.chars().any(char::is_control) {
                    return Err(crate::error::invalid("Invalid folder"));
                }
            }
            Self::Cancel { run, yes } => {
                acknowledge(*yes)?;
                positive(*run)?;
            }
            Self::View { id } | Self::Runs { id } | Self::Run { id, .. } => positive(*id)?,
            Self::Inspect { run }
            | Self::Logs { run, .. }
            | Self::Watch { run }
            | Self::Retry { run } => positive(*run)?,
            Self::Rerun { id, run } => {
                positive(*id)?;
                positive(*run)?;
            }
            Self::Register { name, repo, yaml } => {
                nonempty(name, "Pipeline name")?;
                repository(repo)?;
                nonempty(yaml, "YAML path")?;
            }
            Self::List => (),
        }
        if let Self::Logs { log, start, .. } = self {
            positive(*log)?;
            if *start > i64::MAX as u64 - 1999 {
                return Err(crate::error::invalid("Log offset too large"));
            }
        }
        if let Self::Run {
            branch: Some(branch),
            ..
        } = self
        {
            nonempty(branch, "Branch")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_supported_without_interactive_cli() {
        assert!(Cli::try_parse_from(["devops", "pr", "list", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["devops", "auth", "login"]).is_err());
    }
    #[test]
    fn destructive_commands_require_acknowledgement() {
        assert!(PrCommand::Abandon { id: 1, yes: false }.validate().is_err());
        assert!(acknowledge(true).is_ok());
    }
}
