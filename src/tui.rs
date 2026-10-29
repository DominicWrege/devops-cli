use crate::{
    auth,
    cli::*,
    config::{AuthMethod, Settings, Store, Target},
    output::{self, scalar, text},
    pipeline::{LogKey, Viewer},
    service::{Service, build_summary, response_id},
};
use anyhow::{Context, Result, ensure};
use clap::ValueEnum;
use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    prelude::*,
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
    },
};
use serde_json::Value;
use std::{
    io::{self, IsTerminal},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

type Terminal = ratatui::Terminal<CrosstermBackend<io::Stdout>>;

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> Result<(Self, Terminal)> {
        ensure!(
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            "The TUI requires an interactive terminal"
        );
        enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok((guard, terminal))
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            Show
        );
    }
}

enum Screen {
    Home,
    Organizations(Vec<Value>),
    Projects(Vec<Value>),
    Prs(Vec<Value>),
    Pr { value: Value },
    Pipelines(Vec<Value>),
    Recent(Vec<Value>),
    Runs { pipeline: i32, values: Vec<Value> },
    Watch(Box<Viewer>),
}
enum Message {
    LoginProgress(String),
    Organizations(Vec<Value>),
    Projects(Vec<Value>),
    Prs(Vec<Value>),
    Pr {
        value: Value,
    },
    Pipelines(Vec<Value>),
    Recent(Vec<Value>),
    Runs {
        pipeline: i32,
        values: Vec<Value>,
    },
    Snapshot {
        run: Value,
        records: Vec<Value>,
        log: Option<(LogKey, u64, Vec<String>)>,
        log_error: Option<String>,
    },
    Activity {
        pipeline: i32,
        values: Vec<Value>,
    },
    Written(String),
    Error(String),
}

enum FormKind {
    Identity,
    PatIdentity,
    Create,
    Edit(i32),
    Comment(i32),
    Merge(i32),
}
struct Field {
    name: &'static str,
    value: String,
    secret: bool,
}
impl Field {
    fn display(&self) -> String {
        if self.secret {
            "*".repeat(self.value.chars().count())
        } else {
            output::clean(&self.value)
        }
    }
}
struct Form {
    kind: FormKind,
    fields: Vec<Field>,
    selected: usize,
}
impl Form {
    fn new(kind: FormKind, fields: Vec<(&'static str, String)>) -> Self {
        let pat = matches!(kind, FormKind::PatIdentity);
        Self {
            kind,
            fields: fields
                .into_iter()
                .enumerate()
                .map(|(i, (name, value))| Field {
                    name,
                    value,
                    secret: pat && i == 1,
                })
                .collect(),
            selected: 0,
        }
    }
    fn value(&self, i: usize) -> Result<&str> {
        self.fields
            .get(i)
            .map(|field| field.value.as_str())
            .context("Form field is missing")
    }
    fn selected_field(&mut self) -> Result<&mut Field> {
        self.fields
            .get_mut(self.selected)
            .context("No field selected")
    }
    fn command(&self) -> Result<PrCommand> {
        let list = |i: usize| -> Result<Vec<String>> {
            Ok(self
                .value(i)?
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect())
        };
        Ok(match self.kind {
            FormKind::Create => PrCommand::Create(PrCreate {
                repo: self.value(0)?.into(),
                source: self.value(1)?.into(),
                target: self.value(2)?.into(),
                title: self.value(3)?.into(),
                description: self.value(4)?.into(),
                draft: self
                    .value(5)?
                    .parse()
                    .context("Draft must be true or false")?,
                reviewer: list(6)?,
            }),
            FormKind::Edit(id) => PrCommand::Edit(PrEdit {
                id,
                title: Some(self.value(0)?.into()),
                description: Some(self.value(1)?.into()),
                draft: Some(
                    self.value(2)?
                        .parse()
                        .context("Draft must be true or false")?,
                ),
                add_reviewer: list(3)?,
                remove_reviewer: list(4)?,
            }),
            FormKind::Comment(id) => PrCommand::Comment {
                id,
                body: self.value(0)?.into(),
            },
            FormKind::Merge(id) => PrCommand::Merge {
                id,
                strategy: MergeStrategy::from_str(self.value(0)?, true)
                    .map_err(|e| anyhow::anyhow!(e))?,
                yes: true,
            },
            FormKind::Identity | FormKind::PatIdentity => anyhow::bail!("Not a PR action"),
        })
    }
}

struct LiveSession {
    task: JoinHandle<()>,
    rx: mpsc::Receiver<crate::live::Event>,
}
impl Drop for LiveSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct App {
    store: Store,
    settings: Settings,
    org: Option<String>,
    project: Option<String>,
    setup_org: Option<String>,
    attention: bool,
    screen: Screen,
    selection: ListState,
    detail_scroll: u16,
    pr_tab: usize,
    pane_focus: usize,
    pane_x: [u16; 3],
    pane_y: [u16; 3],
    form: Option<Form>,
    confirm: Option<PrCommand>,
    settings_menu: Option<usize>,
    status: String,
    task: Option<JoinHandle<()>>,
    writing: bool,
    loading_since: Option<Instant>,
    loading_label: &'static str,
    activity: Vec<Value>,
    activity_pipeline: Option<i32>,
    pr_cache: Option<(Target, bool, Vec<Value>)>,
    pipeline_cache: Option<(Target, Vec<Value>)>,
    activity_cache: std::collections::HashMap<String, Vec<Value>>,
    pr_selection: usize,
    pipeline_selection: usize,
    activity_due: Option<Instant>,
    recent_due: Instant,
    tx: mpsc::UnboundedSender<(u64, Message)>,
    generation: u64,
    poll_at: Instant,
    live: Option<LiveSession>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.cancel_task();
    }
}

impl App {
    fn new(
        store: Store,
        settings: Settings,
        org: Option<String>,
        project: Option<String>,
        tx: mpsc::UnboundedSender<(u64, Message)>,
    ) -> Self {
        Self {
            store,
            settings,
            org,
            project,
            setup_org: None,
            attention: false,
            screen: Screen::Home,
            selection: ListState::default().with_selected(Some(0)),
            detail_scroll: 0,
            pr_tab: 0,
            pane_focus: 0,
            pane_x: [0; 3],
            pane_y: [0; 3],
            form: None,
            confirm: None,
            settings_menu: None,
            status: String::new(),
            task: None,
            writing: false,
            loading_since: None,
            loading_label: "Loading",
            activity: Vec::new(),
            activity_pipeline: None,
            pr_cache: None,
            pipeline_cache: None,
            activity_cache: std::collections::HashMap::new(),
            pr_selection: 0,
            pipeline_selection: 0,
            activity_due: None,
            recent_due: Instant::now(),
            tx,
            generation: 0,
            poll_at: Instant::now(),
            live: None,
        }
    }
    fn start(&mut self) -> Result<()> {
        let mut identity = self.settings.clone();
        if let Some(org) = &self.org {
            identity.organization = Some(crate::config::organization(org)?);
        }
        if auth::Credential::new(self.store.clone(), identity).is_err() {
            self.identity_form();
        } else if self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())
            .is_err()
        {
            self.choose_project()?;
        } else {
            self.open_workspace();
        }
        Ok(())
    }
    fn open_workspace(&mut self) {
        self.set_screen(Screen::Prs(Vec::new()));
        if let Err(error) = self.load_prs() {
            self.status = output::clean(&error.to_string());
        }
    }
    fn choose_project(&mut self) -> Result<()> {
        let organization = crate::config::organization(
            self.org
                .as_deref()
                .or(self.settings.organization.as_deref())
                .context("Sign in first with s")?,
        )?;
        let mut identity = self.settings.clone();
        identity.organization = Some(organization.clone());
        let api = Service::new(
            auth::Credential::new(self.store.clone(), identity)?,
            Target {
                organization: organization.clone(),
                project: String::new(),
            },
        )?;
        self.setup_org = Some(organization);
        self.launch(async move { Ok(Message::Projects(api.projects().await?)) });
        Ok(())
    }
    fn api(&self) -> Result<Service> {
        let target = self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())?;
        let mut identity = self.settings.clone();
        identity.organization = Some(target.organization.clone());
        Service::new(auth::Credential::new(self.store.clone(), identity)?, target)
    }
    fn setup_api(&self) -> Result<Service> {
        Service::new(
            auth::Credential::new(self.store.clone(), self.settings.clone())?,
            Target {
                organization: self.settings.organization.clone().unwrap_or_default(),
                project: String::new(),
            },
        )
    }
    fn cancel_task(&mut self) {
        self.loading_since = None;
        self.activity_due = None;
        self.writing = false;
        self.generation = self.generation.wrapping_add(1);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
    fn launch<F>(&mut self, future: F)
    where
        F: std::future::Future<Output = Result<Message>> + Send + 'static,
    {
        self.cancel_task();
        let generation = self.generation;
        let tx = self.tx.clone();
        self.status = "Loading...".into();
        self.loading_since = Some(Instant::now());
        self.loading_label = match self.screen {
            Screen::Prs(_) => "Loading pull requests",
            Screen::Pipelines(_) => "Loading pipelines",
            Screen::Recent(_) => "Refreshing recent runs",
            Screen::Watch(_) => "Updating timeline and logs",
            _ => "Loading from Azure DevOps",
        };
        self.task = Some(tokio::spawn(async move {
            let message = match future.await {
                Ok(message) => message,
                Err(e) => Message::Error(output::clean(&e.to_string())),
            };
            let _ = tx.send((generation, message));
        }));
    }
    fn set_screen(&mut self, screen: Screen) {
        self.live = None;
        match self.screen {
            Screen::Prs(_) => self.pr_selection = self.selection.selected().unwrap_or(0),
            Screen::Pipelines(_) => {
                self.pipeline_selection = self.selection.selected().unwrap_or(0)
            }
            _ => {}
        }
        let selected = match &screen {
            Screen::Prs(items) => self.pr_selection.min(items.len().saturating_sub(1)),
            Screen::Pipelines(items) => self.pipeline_selection.min(items.len().saturating_sub(1)),
            _ => 0,
        };
        self.screen = screen;
        self.selection = ListState::default().with_selected(Some(selected));
        self.detail_scroll = 0;
        self.pr_tab = 0;
        self.pane_focus = 0;
        self.pane_x = [0; 3];
        self.pane_y = [0; 3];
    }
    fn identity_form(&mut self) {
        if auth::uses_pat(&self.settings) {
            self.form = Some(Form::new(
                FormKind::PatIdentity,
                vec![
                    (
                        "Organization",
                        self.org
                            .clone()
                            .or_else(|| self.settings.organization.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "Personal access token (blank: keep saved token / DEVOPS_PAT)",
                        String::new(),
                    ),
                ],
            ));
            return;
        }
        self.form = Some(Form::new(
            FormKind::Identity,
            vec![
                (
                    "Tenant ID",
                    self.settings.tenant_id.clone().unwrap_or_default(),
                ),
                (
                    "Public-client app ID",
                    self.settings.client_id.clone().unwrap_or_default(),
                ),
            ],
        ));
    }
    fn sign_in(&mut self) -> Result<()> {
        if auth::uses_pat(&self.settings) {
            let mut api = self.setup_api()?;
            if let Some(org) = &self.setup_org {
                api.target.organization = org.clone();
            }
            self.setup_org = Some(api.target.organization.clone());
            self.launch(async move { Ok(Message::Projects(api.projects().await?)) });
            return Ok(());
        }
        self.settings.identity()?;
        let store = self.store.clone();
        let settings = self.settings.clone();
        let tx = self.tx.clone();
        let generation = self.generation.wrapping_add(1);
        let api = self.setup_api()?;
        self.launch(async move {
            let (notify, mut rx) = mpsc::unbounded_channel();
            let forward = tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    let _ = tx.send((generation, Message::LoginProgress(message)));
                }
            });
            let result = auth::login(&store, &settings, notify).await;
            forward.await?;
            result?;
            Ok(Message::Organizations(api.organizations().await?))
        });
        Ok(())
    }
    fn load_prs(&mut self) -> Result<()> {
        let target = self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())?;
        if let Some((cached_target, attention, items)) = &self.pr_cache
            && *cached_target == target
            && *attention == self.attention
        {
            let items = items.clone();
            self.cancel_task();
            self.set_screen(Screen::Prs(items));
            self.status = "F5 refreshes this view".into();
            return Ok(());
        }
        let api = self.api()?;
        let attention = self.attention;
        if !matches!(self.screen, Screen::Prs(_)) {
            self.set_screen(Screen::Prs(Vec::new()));
        }
        self.launch(async move {
            Ok(Message::Prs(if attention {
                api.attention_prs().await?
            } else {
                api.pr_list(PrState::Active, None, None, None).await?
            }))
        });
        Ok(())
    }
    fn load_recent(&mut self) -> Result<()> {
        let api = self.api()?;
        let target = self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())?;
        let pipelines = self
            .pipeline_cache
            .as_ref()
            .filter(|(cached, _)| *cached == target)
            .map(|(_, values)| values.clone());
        if !matches!(self.screen, Screen::Recent(_)) {
            self.set_screen(Screen::Recent(Vec::new()));
        }
        self.recent_due = Instant::now() + Duration::from_secs(16);
        self.launch(async move {
            let pipelines = match pipelines {
                Some(values) => values,
                None => api.pipeline_list().await?,
            };
            Ok(Message::Recent(api.recent_runs(&pipelines).await?))
        });
        Ok(())
    }
    fn load_pipelines(&mut self) -> Result<()> {
        self.load_recent()
    }
    fn load_all_pipelines(&mut self) -> Result<()> {
        let target = self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())?;
        if let Some((cached_target, items)) = &self.pipeline_cache
            && *cached_target == target
        {
            let items = items.clone();
            self.cancel_task();
            self.set_screen(Screen::Pipelines(items));
            self.status = "F5 refreshes this view".into();
            return self.load_activity();
        }
        let api = self.api()?;
        if !matches!(self.screen, Screen::Pipelines(_)) {
            self.set_screen(Screen::Pipelines(Vec::new()));
            self.activity.clear();
            self.activity_pipeline = None;
        }
        self.launch(async move { Ok(Message::Pipelines(api.pipeline_list().await?)) });
        Ok(())
    }
    fn activity_key(&self, pipeline: i32) -> Result<String> {
        let target = self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())?;
        Ok(format!(
            "{}/{}/{pipeline}",
            target.organization, target.project
        ))
    }
    fn load_activity(&mut self) -> Result<()> {
        let Screen::Pipelines(items) = &self.screen else {
            return Ok(());
        };
        let Some(item) = items.get(self.selection.selected().unwrap_or(0)) else {
            return Ok(());
        };
        let pipeline = response_id(item, "id")?;
        let key = self.activity_key(pipeline)?;
        if let Some(values) = self.activity_cache.get(&key) {
            let values = values.clone();
            self.cancel_task();
            self.activity = values;
            self.activity_pipeline = Some(pipeline);
            self.status = "F5 refreshes this view".into();
            return Ok(());
        }
        let api = self.api()?;
        self.activity.clear();
        self.activity_pipeline = Some(pipeline);
        self.launch(async move {
            Ok(Message::Activity {
                pipeline,
                values: api.runs(pipeline).await?,
            })
        });
        self.loading_label = "Loading pipeline activity";
        Ok(())
    }
    fn load_pr(&mut self, id: i32) -> Result<()> {
        let api = self.api()?;
        self.launch(async move {
            let (value, _) = api.pr_details(id).await?;
            Ok(Message::Pr { value })
        });
        Ok(())
    }
    fn load_runs(&mut self, pipeline: i32) -> Result<()> {
        let api = self.api()?;
        self.launch(async move {
            Ok(Message::Runs {
                pipeline,
                values: api.runs(pipeline).await?,
            })
        });
        Ok(())
    }
    fn execute_pr(&mut self, command: PrCommand) -> Result<()> {
        let api = self.api()?;
        self.launch(async move { Ok(Message::Written(api.pr_action(&command).await?)) });
        self.writing = true;
        self.loading_label = "Saving changes";
        Ok(())
    }
    fn sync_live(&mut self) -> Result<()> {
        let Screen::Watch(viewer) = &mut self.screen else {
            self.live = None;
            return Ok(());
        };
        if let Some(live) = &mut self.live {
            while let Ok(event) = live.rx.try_recv() {
                viewer.accept_live(event);
            }
        }
        if viewer.run["status"] == "completed" {
            self.live = None;
            viewer.live_status = "Run completed; fetching uploaded logs".into();
            return Ok(());
        }
        if self.live.is_none()
            && let Some(project) = viewer.run["project"]["id"].as_str()
        {
            let project = project.to_string();
            let run = viewer.run_id;
            let client = self.api()?.live();
            let (tx, rx) = mpsc::channel(64);
            self.live = Some(LiveSession {
                task: tokio::spawn(client.run(project, run, tx)),
                rx,
            });
        }
        Ok(())
    }
    fn poll(&mut self) -> Result<()> {
        let Screen::Watch(viewer) = &self.screen else {
            return Ok(());
        };
        let background = !viewer.run.is_null();
        let previous_status = self.status.clone();
        let id = viewer.run_id;
        let key = viewer.log_key.clone();
        let start = viewer.log.next;
        let api = self.api()?;
        self.poll_at = Instant::now() + Duration::from_secs(2);
        // Network work runs outside the input/render loop. Every fetch is bounded to 2,000 lines.
        self.launch(async move {
            let (run, records) = tokio::try_join!(api.build_run(id), api.timeline(id))?;
            let (log, log_error) = if let Some(key) = key {
                match api.log(id, key.log, start).await {
                    Ok(lines) => (Some((key, start, lines)), None),
                    Err(error) => (None, Some(error.to_string())),
                }
            } else {
                (None, None)
            };
            Ok(Message::Snapshot {
                run,
                records,
                log,
                log_error,
            })
        });
        if background {
            self.loading_since = None;
            self.status = previous_status;
        }
        Ok(())
    }
    fn receive(&mut self, (generation, message): (u64, Message)) {
        if generation != self.generation {
            return;
        }
        if let Message::LoginProgress(status) = message {
            self.status = status;
            return;
        }
        self.task = None;
        self.loading_since = None;
        self.writing = false;
        self.status = String::new();
        match message {
            Message::Organizations(items) => self.set_screen(Screen::Organizations(items)),
            Message::Projects(items) => {
                let selected = if self.setup_org == self.settings.organization {
                    items
                        .iter()
                        .position(|v| Some(text(v, "name")) == self.settings.project.as_deref())
                        .unwrap_or(0)
                } else {
                    0
                };
                self.set_screen(Screen::Projects(items));
                self.selection.select(Some(selected));
            }
            Message::Recent(items) => {
                if let Screen::Recent(previous) = &self.screen {
                    let selected_id = previous
                        .get(self.selection.selected().unwrap_or(0))
                        .and_then(|v| v["id"].as_i64());
                    let selected = selected_id
                        .and_then(|id| items.iter().position(|v| v["id"].as_i64() == Some(id)))
                        .unwrap_or(0);
                    self.screen = Screen::Recent(items);
                    self.selection.select(Some(selected));
                    self.status.clear();
                }
            }
            Message::Prs(items) => {
                if let Ok(target) = self
                    .settings
                    .target(self.org.as_deref(), self.project.as_deref())
                {
                    self.pr_cache = Some((target, self.attention, items.clone()));
                }
                self.set_screen(Screen::Prs(items));
            }
            Message::Pr { value } => self.set_screen(Screen::Pr { value }),
            Message::Pipelines(mut items) => {
                sort_pipelines(&mut items);
                if let Ok(target) = self
                    .settings
                    .target(self.org.as_deref(), self.project.as_deref())
                {
                    self.pipeline_cache = Some((target, items.clone()));
                }
                self.set_screen(Screen::Pipelines(items));
                if let Err(error) = self.load_activity() {
                    self.status = error.to_string();
                }
            }
            Message::Activity { pipeline, values } => {
                if matches!(self.screen, Screen::Pipelines(_))
                    && self.activity_pipeline == Some(pipeline)
                {
                    if let Ok(key) = self.activity_key(pipeline) {
                        self.activity_cache.insert(key, values.clone());
                    }
                    self.activity = values;
                }
            }
            Message::Runs { pipeline, values } => {
                self.set_screen(Screen::Runs { pipeline, values })
            }
            Message::Snapshot {
                run,
                records,
                log,
                log_error,
            } => {
                if let Screen::Watch(viewer) = &mut self.screen {
                    viewer.error = log_error.unwrap_or_default();
                    self.status = viewer.error.clone();
                    viewer.update(run, records);
                    if let Some((key, start, lines)) = log {
                        viewer.accept_log(key, start, lines);
                    }
                }
            }
            Message::Written(message) => {
                self.pr_cache = None;
                self.status = format!("{message}\nPress F5 to refresh.");
            }
            Message::Error(message) => {
                if let Screen::Watch(viewer) = &mut self.screen {
                    viewer.error = message.clone();
                }
                self.status = message;
            }
            Message::LoginProgress(_) => (),
        }
    }
    fn submit(&mut self) -> Result<()> {
        let form = self.form.as_ref().context("No form")?;
        if matches!(form.kind, FormKind::PatIdentity) {
            let organization = crate::config::organization(form.value(0)?)?;
            let entered = form.value(1)?;
            let environment = auth::pat::environment()?;
            if !entered.is_empty() && environment.is_some() {
                return Err(crate::error::invalid(
                    "Unset DEVOPS_PAT before replacing the saved token, or leave the token field blank to use the environment override.",
                ));
            }
            let persist = !entered.is_empty();
            let token = if persist {
                auth::pat::validate(entered)?;
                entered.to_owned()
            } else {
                match environment {
                    Some(token) => token,
                    None => auth::pat::load(&self.store, &organization)?
                        .ok_or_else(auth::pat::required)?,
                }
            };
            let api = Service::new(
                auth::Credential::from_pat(token.clone())?,
                Target {
                    organization: organization.clone(),
                    project: String::new(),
                },
            )?;
            let store = self.store.clone();
            // Commit the new organization only after the user selects its project.
            self.setup_org = Some(organization.clone());
            self.form = None;
            self.launch(async move {
                let projects = api.projects().await?;
                if persist {
                    auth::pat::save(&store, &organization, &token)?;
                }
                Ok(Message::Projects(projects))
            });
            return Ok(());
        }
        if matches!(form.kind, FormKind::Identity) {
            let mut settings = self.settings.clone();
            settings.tenant_id = Some(form.value(0)?.trim().into());
            settings.client_id = Some(form.value(1)?.trim().into());
            settings.identity()?;
            self.store.save(&settings)?;
            self.settings = settings;
            self.form = None;
            return self.sign_in();
        }
        let command = form.command()?;
        command.validate()?;
        self.form = None;
        if matches!(command, PrCommand::Merge { .. } | PrCommand::Abandon { .. }) {
            self.confirm = Some(command);
        } else {
            self.execute_pr(command)?;
        }
        Ok(())
    }
    fn enter(&mut self) -> Result<()> {
        let index = self.selection.selected().unwrap_or(0);
        match &self.screen {
            Screen::Organizations(items) => {
                let org = items.get(index).context("No organizations available")?;
                let organization = crate::config::organization(text(org, "accountName"))?;
                self.setup_org = Some(organization.clone());
                // Do not save an old project under a newly selected organization.
                let mut api = self.setup_api()?;
                api.target.organization = organization;
                self.launch(async move { Ok(Message::Projects(api.projects().await?)) });
            }
            Screen::Projects(items) => {
                let project = items.get(index).context("No projects available")?;
                let mut settings = self.settings.clone();
                settings.organization = self.setup_org.clone();
                settings.project = Some(text(project, "name").into());
                settings.project_id = project["id"].as_str().map(str::to_string);
                if auth::uses_pat(&settings) {
                    settings.auth = AuthMethod::Pat;
                }
                settings.target(None, None)?;
                self.store.save(&settings)?;
                self.org = None;
                self.project = None;
                self.settings = settings;
                self.setup_org = None;
                self.open_workspace();
            }
            Screen::Prs(items) => {
                if let Some(item) = items.get(index) {
                    self.load_pr(response_id(item, "pullRequestId")?)?;
                }
            }
            Screen::Pipelines(items) => {
                if let Some(item) = items.get(index) {
                    self.load_runs(response_id(item, "id")?)?;
                }
            }
            Screen::Recent(values) | Screen::Runs { values, .. } => {
                if let Some(item) = values.get(index) {
                    let run = response_id(item, "id")?;
                    self.set_screen(Screen::Watch(Box::new(Viewer::new(run))));
                    self.poll_at = Instant::now();
                }
            }
            _ => (),
        }
        Ok(())
    }
    fn open_pr_form(&mut self, key: char) -> Result<()> {
        if key == 'n' {
            self.form = Some(Form::new(
                FormKind::Create,
                vec![
                    ("Repository", String::new()),
                    ("Source branch", String::new()),
                    ("Target branch", "main".into()),
                    ("Title", String::new()),
                    ("Description", String::new()),
                    ("Draft, true/false", "false".into()),
                    ("Reviewer UUIDs, comma separated", String::new()),
                ],
            ));
            return Ok(());
        }
        let Screen::Pr { value, .. } = &self.screen else {
            return Ok(());
        };
        let id = response_id(value, "pullRequestId")?;
        match key {
            'e' => {
                self.form = Some(Form::new(
                    FormKind::Edit(id),
                    vec![
                        ("Title", text(value, "title").into()),
                        ("Description", text(value, "description").into()),
                        ("Draft, true/false", scalar(&value["isDraft"])),
                        ("Add reviewer UUIDs", String::new()),
                        ("Remove reviewer UUIDs", String::new()),
                    ],
                ))
            }
            'c' => {
                self.form = Some(Form::new(
                    FormKind::Comment(id),
                    vec![("Comment", String::new())],
                ))
            }
            'm' => {
                self.form = Some(Form::new(
                    FormKind::Merge(id),
                    vec![(
                        "squash / no-fast-forward / rebase / rebase-merge",
                        "squash".into(),
                    )],
                ))
            }
            'a' => self.confirm = Some(PrCommand::Abandon { id, yes: true }),
            'r' => self.execute_pr(PrCommand::Reactivate { id })?,
            _ => (),
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        self.pr_cache = None;
        self.pipeline_cache = None;
        self.activity_cache.clear();
        match &self.screen {
            Screen::Recent(_) => self.load_recent()?,
            Screen::Prs(_) => self.load_prs()?,
            Screen::Pr { value, .. } => self.load_pr(response_id(value, "pullRequestId")?)?,
            Screen::Pipelines(_) => self.load_all_pipelines()?,
            Screen::Runs { pipeline, .. } => {
                let id = *pipeline;
                self.load_runs(id)?;
            }
            Screen::Watch(_) => self.poll_at = Instant::now(),
            _ => (),
        }
        Ok(())
    }
    fn handle(&mut self, event: Event) -> Result<bool> {
        if let Event::Paste(paste) = &event {
            if let Some(form) = &mut self.form {
                form.selected_field()?.value.push_str(&output::clean(paste));
            }
            return Ok(false);
        }
        let Event::Key(key) = event else {
            return Ok(false);
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Ok(false);
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        if let Some(command) = self.confirm.take() {
            match key.code {
                KeyCode::Char('y') => self.execute_pr(command)?,
                KeyCode::Char('n') | KeyCode::Esc => self.status = "Canceled".into(),
                _ => self.confirm = Some(command),
            }
            return Ok(false);
        }
        if let Some(form) = &mut self.form {
            ensure!(!form.fields.is_empty(), "Form has no fields");
            match key.code {
                KeyCode::Esc => self.form = None,
                KeyCode::Tab | KeyCode::Down => {
                    form.selected = (form.selected + 1) % form.fields.len()
                }
                KeyCode::BackTab | KeyCode::Up => {
                    form.selected = (form.selected + form.fields.len() - 1) % form.fields.len()
                }
                KeyCode::Backspace => {
                    form.selected_field()?.value.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    form.selected_field()?.value.clear()
                }
                KeyCode::Char(c) if !c.is_control() => form.selected_field()?.value.push(c),
                KeyCode::Enter => self.submit()?,
                _ => (),
            }
            return Ok(false);
        }
        if let Some(selected) = self.settings_menu {
            match key.code {
                KeyCode::Esc | KeyCode::Char('s') => self.settings_menu = None,
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Char('j' | 'k') => {
                    self.settings_menu = Some(1 - selected);
                }
                KeyCode::Enter => {
                    self.settings_menu = None;
                    if selected == 0 {
                        self.choose_project()?;
                    } else {
                        self.identity_form();
                    }
                }
                KeyCode::Char('q') => return Ok(true),
                _ => {}
            }
            return Ok(false);
        }
        if key.code == KeyCode::Char('s') && !self.writing {
            self.cancel_task();
            self.settings_menu = Some(0);
            return Ok(false);
        }
        if key.code == KeyCode::Char('q') {
            return Ok(true);
        }
        if key.code == KeyCode::F(6) {
            let panes = match self.screen {
                Screen::Watch(_) => 3,
                Screen::Prs(_) | Screen::Pr { .. } | Screen::Pipelines(_) => 2,
                _ => 1,
            };
            self.pane_focus = (self.pane_focus + 1) % panes;
            return Ok(false);
        }
        if matches!(self.screen, Screen::Pr { .. }) {
            match key.code {
                KeyCode::Char('[' | ']') => {
                    self.pr_tab =
                        (self.pr_tab + if key.code == KeyCode::Char(']') { 1 } else { 2 }) % 3;
                    self.pane_y = [0; 3];
                    return Ok(false);
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
                    let step = if key.code == KeyCode::PageUp { 20 } else { 1 };
                    self.pane_y[self.pane_focus] =
                        self.pane_y[self.pane_focus].saturating_sub(step);
                    return Ok(false);
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                    let step = if key.code == KeyCode::PageDown { 20 } else { 1 };
                    self.pane_y[self.pane_focus] =
                        self.pane_y[self.pane_focus].saturating_add(step);
                    return Ok(false);
                }
                KeyCode::Home => {
                    self.pane_y[self.pane_focus] = 0;
                    return Ok(false);
                }
                _ => {}
            }
        }
        if matches!(key.code, KeyCode::Left | KeyCode::Right) {
            let offset = &mut self.pane_x[self.pane_focus];
            *offset = if key.code == KeyCode::Left {
                offset.saturating_sub(8)
            } else {
                offset.saturating_add(8)
            };
            return Ok(false);
        }
        // Switch views before the loading guard; never interrupt a submitted write.
        if matches!(
            key.code,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('1' | '2')
        ) && !self.writing
            && matches!(
                self.screen,
                Screen::Prs(_)
                    | Screen::Pr { .. }
                    | Screen::Pipelines(_)
                    | Screen::Recent(_)
                    | Screen::Runs { .. }
                    | Screen::Watch(_)
            )
        {
            let pipelines = match key.code {
                KeyCode::Char('1') => false,
                KeyCode::Char('2') => true,
                _ => matches!(self.screen, Screen::Prs(_) | Screen::Pr { .. }),
            };
            if pipelines {
                self.load_pipelines()?;
            } else {
                self.load_prs()?;
            }
            return Ok(false);
        }
        if key.code == KeyCode::Esc && self.task.is_some() && !self.writing {
            self.cancel_task();
            self.status = "Loading canceled. F5 to retry.".into();
            return Ok(false);
        }
        // Keep navigation responsive while polling, but don't overlap writes or setup.
        if self.task.is_some()
            && !matches!(
                self.screen,
                Screen::Watch(_) | Screen::Pipelines(_) | Screen::Recent(_)
            )
        {
            return Ok(false);
        }
        if let Screen::Watch(viewer) = &mut self.screen {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') if self.pane_focus == 0 => viewer.navigate(-1),
                KeyCode::Down | KeyCode::Char('j') if self.pane_focus == 0 => viewer.navigate(1),
                KeyCode::Up | KeyCode::Char('k') if self.pane_focus == 1 => viewer.scroll(-1),
                KeyCode::Down | KeyCode::Char('j') if self.pane_focus == 1 => viewer.scroll(1),
                KeyCode::PageUp if self.pane_focus == 0 => viewer.navigate(-20),
                KeyCode::PageDown if self.pane_focus == 0 => viewer.navigate(20),
                KeyCode::PageUp if self.pane_focus == 1 => viewer.scroll(-20),
                KeyCode::PageDown if self.pane_focus == 1 => viewer.scroll(20),
                KeyCode::Up | KeyCode::PageUp => self.pane_y[2] = self.pane_y[2].saturating_sub(1),
                KeyCode::Down | KeyCode::PageDown => {
                    self.pane_y[2] = self.pane_y[2].saturating_add(1)
                }
                KeyCode::Char('f') => viewer.resume(),
                KeyCode::Esc => {
                    self.cancel_task();
                    // The generation check rejects replies already queued before abort.
                    self.set_screen(Screen::Home);
                }
                _ => (),
            }
            return Ok(false);
        }
        if self.pane_focus == 0 && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            let count = match &self.screen {
                Screen::Prs(items)
                | Screen::Pipelines(items)
                | Screen::Recent(items)
                | Screen::Projects(items)
                | Screen::Organizations(items) => Some(items.len()),
                Screen::Runs { values, .. } => Some(values.len()),
                _ => None,
            };
            if let Some(count) = count {
                let selected = self.selection.selected().unwrap_or(0);
                let selected = if key.code == KeyCode::PageUp {
                    selected.saturating_sub(20)
                } else {
                    selected.saturating_add(20).min(count.saturating_sub(1))
                };
                self.selection.select(Some(selected));
                if matches!(self.screen, Screen::Pipelines(_)) {
                    self.load_activity()?;
                }
                return Ok(false);
            }
        }
        if self.pane_focus == 1 && matches!(self.screen, Screen::Prs(_) | Screen::Pipelines(_)) {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.detail_scroll = self.detail_scroll.saturating_add(1)
                }
                KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(20),
                KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(20),
                KeyCode::Home => self.detail_scroll = 0,
                _ => {}
            }
            if matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Char('j' | 'k')
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Home
            ) {
                return Ok(false);
            }
        }
        match key.code {
            KeyCode::Char('r')
                if matches!(self.screen, Screen::Pipelines(_) | Screen::Recent(_)) =>
            {
                self.load_recent()?
            }
            KeyCode::Char('a') if matches!(self.screen, Screen::Recent(_)) => {
                self.load_all_pipelines()?
            }
            KeyCode::Char('1') => {
                self.attention = false;
                self.load_prs()?;
            }
            KeyCode::Char('2') => self.load_pipelines()?,
            KeyCode::Esc => match self.screen {
                Screen::Pr { .. } => self.load_prs()?,
                Screen::Runs { .. } => self.load_all_pipelines()?,
                _ => self.set_screen(Screen::Home),
            },
            KeyCode::Char('p') => {
                self.attention = false;
                self.load_prs()?;
            }
            KeyCode::Char('i') if matches!(self.screen, Screen::Home | Screen::Prs(_)) => {
                self.attention = if matches!(self.screen, Screen::Prs(_)) {
                    !self.attention
                } else {
                    true
                };
                self.load_prs()?;
            }
            KeyCode::Char('l') => self.load_pipelines()?,
            KeyCode::F(5) => self.refresh()?,
            KeyCode::Enter => self.enter()?,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selection.select(Some(
                    self.selection.selected().unwrap_or(0).saturating_sub(1),
                ));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = match &self.screen {
                    Screen::Organizations(v)
                    | Screen::Projects(v)
                    | Screen::Prs(v)
                    | Screen::Pipelines(v)
                    | Screen::Recent(v) => v.len(),
                    Screen::Runs { values, .. } => values.len(),
                    _ => 0,
                };
                self.selection.select(Some(
                    (self.selection.selected().unwrap_or(0) + 1).min(count.saturating_sub(1)),
                ));
            }
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(20),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(20),
            KeyCode::Char(c) if matches!(self.screen, Screen::Prs(_) | Screen::Pr { .. }) => {
                self.open_pr_form(c)?
            }
            _ => (),
        }
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k')
        ) && matches!(self.screen, Screen::Pipelines(_))
        {
            let pipeline = if let Screen::Pipelines(items) = &self.screen {
                items
                    .get(self.selection.selected().unwrap_or(0))
                    .map(|v| response_id(v, "id"))
                    .transpose()?
            } else {
                None
            };
            if let Some(pipeline) = pipeline
                && self.activity_pipeline != Some(pipeline)
            {
                self.cancel_task();
                if self
                    .activity_cache
                    .contains_key(&self.activity_key(pipeline)?)
                {
                    self.load_activity()?;
                } else {
                    self.activity.clear();
                    self.activity_pipeline = Some(pipeline);
                    self.status = "Loading activity...".into();
                    self.activity_due = Some(Instant::now() + Duration::from_millis(150));
                }
            }
        }
        Ok(false)
    }
}

pub async fn run(
    store: Store,
    settings: Settings,
    org: Option<String>,
    project: Option<String>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(store, settings, org, project, tx);
    if let Err(error) = app.start() {
        app.status = output::clean(&error.to_string());
    }
    let (_guard, mut terminal) = TerminalGuard::enter()?;
    loop {
        while let Ok(message) = rx.try_recv() {
            app.receive(message);
        }
        if let Err(error) = app.sync_live() {
            app.status = error.to_string();
        }
        if app.task.is_none()
            && matches!(app.screen, Screen::Watch(_))
            && Instant::now() >= app.poll_at
            && let Err(error) = app.poll()
        {
            app.status = error.to_string();
            app.poll_at = Instant::now() + Duration::from_secs(2);
        }
        if matches!(app.screen, Screen::Recent(_))
            && app.task.is_none()
            && app.settings_menu.is_none()
            && Instant::now() >= app.recent_due
            && let Err(error) = app.load_recent()
        {
            app.status = error.to_string();
            app.recent_due = Instant::now() + Duration::from_secs(16);
        }
        if app.activity_due.is_some_and(|due| Instant::now() >= due) {
            app.activity_due = None;
            if let Err(error) = app.load_activity() {
                app.status = error.to_string();
            }
        }
        terminal.draw(|frame| draw(frame, &mut app))?;
        if event::poll(Duration::from_millis(40))? {
            match app.handle(event::read()?) {
                Ok(true) => break,
                Ok(false) => (),
                Err(error) => app.status = output::clean(&error.to_string()),
            }
        }
        // Yield to network tasks even on a single CPU runtime.
        tokio::task::yield_now().await;
    }
    Ok(())
}

fn loading_text(label: &str, elapsed: Duration, writing: bool) -> String {
    let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = frames[((elapsed.as_millis() / 100) % frames.len() as u128) as usize];
    let hint = if writing {
        "Waiting for server confirmation"
    } else {
        "Tab: switch view · Esc: cancel"
    };
    let waiting = if elapsed.as_secs() >= 8 {
        " · Waiting for Azure DevOps"
    } else {
        ""
    };
    format!(
        "{spinner} {label} · {}s{waiting}   {hint}",
        elapsed.as_secs()
    )
}

fn pr_rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn render_pr(
    frame: &mut Frame,
    area: Rect,
    pr: &Value,
    tab: usize,
    focus: usize,
    scroll: [u16; 3],
) {
    let regions = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).split(area);
    let header = format!(
        "#{}  {}\n{}{} | {}\n{} -> {}",
        scalar(&pr["pullRequestId"]),
        text(pr, "title"),
        text(pr, "status"),
        if pr["isDraft"] == true {
            " (draft)"
        } else {
            ""
        },
        text(&pr["createdBy"], "displayName"),
        text(pr, "sourceRefName").trim_start_matches("refs/heads/"),
        text(pr, "targetRefName").trim_start_matches("refs/heads/")
    );
    frame.render_widget(
        Paragraph::new(output::clean(&header)).block(Block::bordered().title("Pull request")),
        regions[0],
    );
    let mut status = Vec::<Line>::new();
    for (heading, key) in [
        ("Reviewers", "reviewers"),
        ("Checks", "detailStatuses"),
        ("Policies", "detailPolicies"),
    ] {
        status.push(Line::styled(heading, Style::default().bold()));
        let rows = pr_rows(&pr[key]);
        if rows.is_empty() {
            status.push(Line::raw("None reported"));
        }
        for row in rows {
            let (name, state) = match key {
                "reviewers" => (
                    text(row, "displayName"),
                    match row["vote"].as_i64().unwrap_or(0) {
                        10 => "Approved",
                        5 => "Approved with suggestions",
                        -5 => "Waiting",
                        -10 => "Changes requested",
                        _ => "Pending",
                    },
                ),
                "detailStatuses" => (text(&row["context"], "name"), text(row, "state")),
                _ => (
                    text(&row["configuration"]["type"], "displayName"),
                    text(row, "status"),
                ),
            };
            let color = match state.to_ascii_lowercase().as_str() {
                "approved" | "approved with suggestions" | "succeeded" => Color::Green,
                "failed" | "error" | "rejected" | "changes requested" => Color::Red,
                _ => Color::Yellow,
            };
            status.push(Line::raw(output::clean(name)));
            status.push(Line::styled(
                format!("  {}", output::clean(state)),
                Style::default().fg(color),
            ));
        }
        status.push(Line::raw(""));
    }
    let mut comments = String::new();
    for thread in pr_rows(&pr["detailThreads"]) {
        for comment in pr_rows(&thread["comments"]) {
            if comment["isDeleted"] == true {
                continue;
            }
            comments.push_str(&format!(
                "{}\n{}\n\n",
                text(&comment["author"], "displayName"),
                text(comment, "content")
            ));
        }
    }
    if comments.is_empty() {
        comments = "No comments yet. Press c to add a comment.".into();
    }
    let description = text(pr, "description");
    let body = match tab {
        1 => Text::raw(output::clean(&comments)),
        2 => Text::from(status.clone()),
        _ => Text::raw(output::clean(if description.is_empty() {
            "No description."
        } else {
            description
        })),
    };
    let wide = area.width >= 90;
    let panes = Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(regions[1]);
    let title = match tab {
        1 => "Description | [Comments] | Status",
        2 => "Description | Comments | [Status]",
        _ => "[Description] | Comments | Status",
    };
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .scroll((scroll[if wide { 0 } else { focus }], 0))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(if focus == 0 {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    })),
            ),
        if wide { panes[0] } else { regions[1] },
    );
    if wide {
        frame.render_widget(
            Paragraph::new(status)
                .wrap(Wrap { trim: false })
                .scroll((scroll[1], 0))
                .block(
                    Block::bordered()
                        .title("Review status")
                        .border_style(Style::default().fg(if focus == 1 {
                            Color::Cyan
                        } else {
                            Color::DarkGray
                        })),
                ),
            panes[1],
        );
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let now = time::OffsetDateTime::now_utc();
    let loading = app
        .loading_since
        .filter(|_| app.task.is_some())
        .map(|since| loading_text(app.loading_label, since.elapsed(), app.writing));
    let status_text = match &loading {
        Some(message) if !app.status.is_empty() && app.status != "Loading..." => {
            format!("{message}\n{}", app.status)
        }
        Some(message) => message.clone(),
        None => app.status.clone(),
    };
    let area = frame.area();
    let layout = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(5),
    ])
    .split(area);
    let context = app
        .settings
        .target(app.org.as_deref(), app.project.as_deref())
        .map(|t| format!("{} / {}", t.organization, t.project))
        .unwrap_or_else(|_| "Setup required".into());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                output::cell(&format!(" DevOps   {context}")),
                Style::default().fg(Color::Cyan).bold(),
            )),
            Line::from(vec![
                Span::styled(
                    " 1 ⑂ Pull requests ",
                    if matches!(app.screen, Screen::Prs(_) | Screen::Pr { .. }) {
                        Style::default().bg(Color::Cyan).fg(Color::Black).bold()
                    } else {
                        Style::default()
                    },
                ),
                Span::raw("  "),
                Span::styled(
                    " 2 ▶ Pipelines ",
                    if matches!(
                        app.screen,
                        Screen::Pipelines(_)
                            | Screen::Recent(_)
                            | Screen::Runs { .. }
                            | Screen::Watch(_)
                    ) {
                        Style::default().bg(Color::Cyan).fg(Color::Black).bold()
                    } else {
                        Style::default()
                    },
                ),
            ]),
        ])
        .block(Block::default().borders(Borders::BOTTOM)),
        layout[0],
    );
    let help = match &mut app.screen {
        Screen::Home => {
            frame.render_widget(Paragraph::new("Your project is remembered between sessions.\n\n1  Pull requests     Review and manage your team's work\n2  Pipelines         Browse runs and follow logs\ns  Settings          Current project and account\nq  Quit\n\nUse Tab to switch between pull requests and pipelines.\nSelect a row with Up/Down or j/k; press Enter to open it.").block(Block::bordered().title("Home")),layout[1]);
            "p: PRs   i: assigned reviews   l: pipelines   s: settings   q: quit"
        }
        Screen::Organizations(items) => {
            draw_list(
                frame,
                layout[1],
                "Select organization",
                items
                    .iter()
                    .map(|v| text(v, "accountName").into())
                    .collect(),
                &mut app.selection,
                app.pane_x[0],
            );
            "Up/Down: select   Enter: continue   Esc: home"
        }
        Screen::Projects(items) => {
            draw_list(
                frame,
                layout[1],
                "Choose current project (remembered next time)",
                items.iter().map(|v| text(v, "name").into()).collect(),
                &mut app.selection,
                app.pane_x[0],
            );
            "Up/Down: select   Enter: use and remember   Esc: cancel"
        }
        Screen::Prs(items) => {
            let panes = if layout[1].width >= 90 {
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(layout[1])
            } else {
                Layout::horizontal([Constraint::Percentage(100)]).split(layout[1])
            };
            if panes.len() > 1 {
                let preview = items
                    .get(app.selection.selected().unwrap_or(0))
                    .map(|v| {
                        format!(
                            "{}\n\n{} -> {}\n\n{}\n\nEnter to open details and actions",
                            text(v, "title"),
                            text(v, "sourceRefName").trim_start_matches("refs/heads/"),
                            text(v, "targetRefName").trim_start_matches("refs/heads/"),
                            text(v, "description")
                        )
                    })
                    .unwrap_or_else(|| {
                        "No active pull requests.\n\nPress n to create one or F5 to refresh.".into()
                    });
                frame.render_widget(
                    Paragraph::new(output::clean(&preview))
                        .wrap(Wrap { trim: false })
                        .block(
                            Block::bordered()
                                .border_style(if app.pane_focus == 1 {
                                    Style::default().fg(Color::Cyan)
                                } else {
                                    Style::default()
                                })
                                .title("Preview"),
                        ),
                    panes[1],
                );
            }
            draw_list(
                frame,
                panes[0],
                if app.attention {
                    "Assigned reviews needing attention"
                } else {
                    "Active pull requests"
                },
                items
                    .iter()
                    .map(|v| {
                        format!(
                            "#{}  {}  {}",
                            scalar(&v["pullRequestId"]),
                            if v["isDraft"] == true { "draft" } else { "" },
                            text(v, "title")
                        )
                    })
                    .collect(),
                &mut app.selection,
                app.pane_x[0],
            );
            "Enter: view   n: create   i: toggle assigned reviews   F5: refresh   Esc: home"
        }
        Screen::Pr { value, .. } => {
            render_pr(
                frame,
                layout[1],
                value,
                app.pr_tab,
                app.pane_focus,
                app.pane_y,
            );
            "e: edit   c: comment   m: merge   a: abandon   r: reactivate   n: create\n[/]: tab   F6: focus   PgUp/PgDn: scroll   F5: refresh   Esc: PR list"
        }
        Screen::Pipelines(items) => {
            let panes = if layout[1].width >= 90 {
                Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .split(layout[1])
            } else {
                Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)])
                    .split(layout[1])
            };
            let name = items
                .get(app.selection.selected().unwrap_or(0))
                .map(|v| text(v, "name"))
                .unwrap_or("Select a pipeline");
            let activity = if app.activity.is_empty() {
                if app.task.is_some() || app.activity_due.is_some() {
                    loading
                        .clone()
                        .unwrap_or_else(|| "Preparing activity...".into())
                } else if !app.status.is_empty() {
                    "Could not load activity. Press F5 to retry.".into()
                } else {
                    "No recent runs. Press Enter to open the run list.".into()
                }
            } else {
                app.activity
                    .iter()
                    .take(20)
                    .map(|run| {
                        format!(
                            "{} #{}  {}  {}  {}\n    {} | Duration: {}",
                            run_icon(run),
                            scalar(&run["id"]),
                            text(run, "name"),
                            text(run, "state"),
                            text(run, "result"),
                            human_timestamp(text(run, "createdDate"), now),
                            run_timer(run, now)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            frame.render_widget(
                Paragraph::new(output::clean(&activity))
                    .wrap(Wrap { trim: false })
                    .scroll((app.detail_scroll, app.pane_x[1]))
                    .block(Block::bordered().title(output::cell(&format!("Activity | {name}")))),
                panes[1],
            );
            let mut previous_folder = String::new();
            let rows = items
                .iter()
                .map(|v| {
                    let folder = pipeline_folder(v);
                    let group = if folder != previous_folder {
                        folder.clone()
                    } else {
                        String::new()
                    };
                    previous_folder = folder;
                    Row::new(
                        vec![
                            output::cell(&group),
                            output::cell(text(v, "name")),
                            scalar(&v["id"]),
                        ]
                        .into_iter()
                        .map(|cell| {
                            cell.chars()
                                .skip(app.pane_x[0] as usize)
                                .collect::<String>()
                        }),
                    )
                })
                .collect();
            draw_table(
                frame,
                panes[0],
                "Pipelines by folder",
                ["Folder", "Pipeline", "ID"].to_vec(),
                rows,
                vec![
                    Constraint::Percentage(30),
                    Constraint::Min(12),
                    Constraint::Length(7),
                ],
                &mut app.selection,
            );
            "r: recent runs (16s refresh)   Up/Down: pipeline   Enter: runs   F5: refresh"
        }
        Screen::Recent(items) => {
            let rows = items
                .iter()
                .map(|run| {
                    let status = if text(run, "result").is_empty() {
                        text(run, "status")
                    } else {
                        text(run, "result")
                    };
                    Row::new(
                        vec![
                            format!("{} {}", run_icon(run), output::cell(status)),
                            output::cell(text(&run["definition"], "name")),
                            output::cell(text(run, "buildNumber")),
                            output::cell(
                                text(run, "sourceBranch").trim_start_matches("refs/heads/"),
                            ),
                            human_timestamp(text(run, "queueTime"), now),
                            run_timer(run, now),
                        ]
                        .into_iter()
                        .map(|cell| {
                            cell.chars()
                                .skip(app.pane_x[0] as usize)
                                .collect::<String>()
                        }),
                    )
                })
                .collect();
            draw_table(
                frame,
                layout[1],
                "Recent runs | newest first | refresh 16s",
                vec!["Status", "Pipeline", "Run", "Branch", "Queued", "Duration"],
                rows,
                vec![
                    Constraint::Length(13),
                    Constraint::Min(12),
                    Constraint::Length(16),
                    Constraint::Percentage(15),
                    Constraint::Length(22),
                    Constraint::Length(12),
                ],
                &mut app.selection,
            );
            "Up/Down: run   Enter: timeline and logs   a: all pipelines   F5: refresh"
        }
        Screen::Runs { values, .. } => {
            let rows = values
                .iter()
                .map(|run| {
                    Row::new(
                        vec![
                            format!(
                                "{} {}",
                                run_icon(run),
                                output::cell(if text(run, "result").is_empty() {
                                    text(run, "state")
                                } else {
                                    text(run, "result")
                                })
                            ),
                            scalar(&run["id"]),
                            output::cell(text(run, "name")),
                            human_timestamp(text(run, "createdDate"), now),
                            run_timer(run, now),
                        ]
                        .into_iter()
                        .map(|cell| {
                            cell.chars()
                                .skip(app.pane_x[0] as usize)
                                .collect::<String>()
                        }),
                    )
                })
                .collect();
            draw_table(
                frame,
                layout[1],
                "Recent runs",
                vec!["Status", "ID", "Run", "Created", "Duration"],
                rows,
                vec![
                    Constraint::Length(13),
                    Constraint::Length(8),
                    Constraint::Min(12),
                    Constraint::Length(22),
                    Constraint::Length(12),
                ],
                &mut app.selection,
            );
            "Enter: observe   F5: refresh   Esc: home"
        }
        Screen::Watch(viewer) => {
            draw_viewer(
                frame,
                layout[1],
                viewer,
                app.pane_focus,
                app.pane_x,
                app.pane_y,
            );
            "F6: tasks / logs / summary   ↑↓ PgUp/PgDn: scroll   ←→: horizontal   f: follow   Esc: back"
        }
    };
    let empty = match &app.screen {
        Screen::Prs(items) | Screen::Pipelines(items) | Screen::Recent(items) => items.is_empty(),
        _ => false,
    };
    if empty && let Some(message) = &loading {
        frame.render_widget(Clear, layout[1]);
        frame.render_widget(
            Paragraph::new(message.as_str())
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Cyan))
                .block(Block::bordered().title("Loading")),
            layout[1],
        );
    }
    let navigation = if matches!(app.screen, Screen::Watch(_)) {
        "s: settings"
    } else {
        "F6: pane   ←→: scroll   s: settings"
    };
    frame.render_widget(
        Paragraph::new(output::clean(&format!(
            "{help}   {navigation}\n{}",
            status_text
        )))
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::TOP)),
        layout[2],
    );
    if let Some(selected) = app.settings_menu {
        let popup = center(area, 65, 45);
        frame.render_widget(Clear, popup);
        let rows = vec![
            format!(
                "Current project   {}",
                app.settings.project.as_deref().unwrap_or("Not selected")
            ),
            "Account           Sign in or replace token".into(),
        ];
        let mut state = ListState::default().with_selected(Some(selected));
        draw_list(
            frame,
            popup,
            "Settings | Enter: open  Esc: close",
            rows,
            &mut state,
            0,
        );
    }
    if let Some(form) = &app.form {
        let popup = center(area, 90, 85);
        frame.render_widget(Clear, popup);
        let mut lines = vec![
            Line::from("Tab/Up/Down: field   Ctrl+U: clear   Enter: submit   Esc: cancel"),
            Line::from(""),
        ];
        for (i, field) in form.fields.iter().enumerate() {
            let style = if i == form.selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            lines.push(Line::styled(
                format!(
                    "{}{}",
                    if i == form.selected { "> " } else { "  " },
                    field.name
                ),
                style,
            ));
            lines.push(Line::styled(format!("  {}", field.display()), style));
            lines.push(Line::from(""));
        }
        let title = if matches!(form.kind, FormKind::Identity | FormKind::PatIdentity) {
            "Sign in and setup"
        } else {
            "Pull request action"
        };
        let scroll =
            (form.selected * 3).saturating_sub(popup.height.saturating_sub(7) as usize) as u16;
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0))
                .block(Block::bordered().title(title)),
            popup,
        );
    }
    if let Some(command) = &app.confirm {
        let operation = match command {
            PrCommand::Merge { id, .. } => format!("Complete/merge PR {id}"),
            PrCommand::Abandon { id, .. } => format!("Abandon PR {id}"),
            _ => String::new(),
        };
        let popup = center(area, 70, 30);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(format!(
                "{operation}?\nTarget: {}\n\ny: confirm   n/Esc: cancel",
                output::cell(&context)
            ))
            .block(Block::bordered().title("Confirm remote write")),
            popup,
        );
    }
}

fn pipeline_folder(value: &Value) -> String {
    let folder = text(value, "folder").trim_matches(['/', '\\']);
    if folder.is_empty() {
        "Root".into()
    } else {
        folder.replace('\\', "/")
    }
}

fn sort_pipelines(items: &mut [Value]) {
    items.sort_by_key(|v| {
        (
            pipeline_folder(v).to_lowercase(),
            text(v, "name").to_lowercase(),
        )
    });
}

fn draw_table(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    headers: Vec<&str>,
    rows: Vec<Row<'static>>,
    widths: Vec<Constraint>,
    selection: &mut ListState,
) {
    let mut state = TableState::default()
        .with_offset(selection.offset())
        .with_selected(selection.selected());
    let rows = if rows.is_empty() {
        vec![Row::new(vec!["No items"])]
    } else {
        rows
    };
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(
                Row::new(headers)
                    .style(Style::default().fg(Color::Cyan).bold())
                    .bottom_margin(1),
            )
            .row_highlight_style(Style::default().bg(Color::DarkGray).fg(Color::Cyan).bold())
            .highlight_symbol("> ")
            .column_spacing(2)
            .block(Block::bordered().title(title)),
        area,
        &mut state,
    );
    *selection.offset_mut() = state.offset();
}

fn run_icon(run: &Value) -> &'static str {
    match (
        if text(run, "state").is_empty() {
            text(run, "status")
        } else {
            text(run, "state")
        },
        text(run, "result"),
    ) {
        (_, "succeeded") => "✓",
        (_, "failed") => "✗",
        (_, "succeededWithIssues" | "partiallySucceeded") => "⚠",
        (_, "skipped") => "−",
        (_, "canceled") => "■",
        ("inProgress", _) => "▶",
        _ => "○",
    }
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<String>,
    state: &mut ListState,
    horizontal: u16,
) {
    let items = if items.is_empty() {
        vec![ListItem::new("No items")]
    } else {
        items
            .into_iter()
            .map(|s| {
                ListItem::new(
                    output::clean(&s)
                        .lines()
                        .map(|line| line.chars().skip(horizontal as usize).collect::<String>())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            })
            .collect()
    };
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("> ")
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::Cyan).bold())
            .block(Block::bordered().title(title)),
        area,
        state,
    );
}
fn human_timestamp(value: &str, now: time::OffsetDateTime) -> String {
    let Ok(timestamp) =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
    else {
        return "Unknown".into();
    };
    let seconds = (now - timestamp).whole_seconds();
    if seconds.abs() < 60 {
        return "Just now".into();
    }
    if seconds.abs() < 7 * 24 * 3600 {
        let (amount, unit) = match seconds.abs() {
            n if n < 3600 => (n / 60, "min"),
            n if n < 86400 => (n / 3600, "h"),
            n => (n / 86400, "d"),
        };
        return if seconds < 0 {
            format!("In {amount}{unit}")
        } else {
            format!("{amount}{unit} ago")
        };
    }
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {} {:02}:{:02} UTC",
        timestamp.day(),
        months[timestamp.month() as usize - 1],
        timestamp.year(),
        timestamp.hour(),
        timestamp.minute()
    )
}

fn run_timer(value: &Value, now: time::OffsetDateTime) -> String {
    let parse = |field: &str| {
        value[field].as_str().and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
    };
    let Some(start) = parse("startTime").or_else(|| parse("createdDate")) else {
        return "Not started".into();
    };
    let end = match parse("finishTime").or_else(|| parse("finishedDate")) {
        Some(end) => end,
        None if value["state"] == "completed"
            || value["status"] == "completed"
            || value["result"].as_str().is_some_and(|r| !r.is_empty()) =>
        {
            return "Duration unavailable".into();
        }
        None => now,
    };
    let seconds = (end - start).whole_seconds().max(0);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn draw_viewer(
    frame: &mut Frame,
    area: Rect,
    viewer: &mut Viewer,
    focus: usize,
    horizontal: [u16; 3],
    vertical: [u16; 3],
) {
    let panes = Layout::vertical([Constraint::Length(8), Constraint::Min(1)]).split(area);
    let now = time::OffsetDateTime::now_utc();
    let elapsed = run_timer(&viewer.run, now);
    frame.render_widget(
        Paragraph::new(output::clean(&format!(
            "{}\nElapsed: {elapsed}",
            build_summary(&viewer.run)
        )))
        .scroll((vertical[2], horizontal[2]))
        .block(
            Block::bordered()
                .border_style(if focus == 2 {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                })
                .title(format!(
                    "{} | Elapsed: {elapsed}",
                    if viewer.follow { "Following" } else { "Paused" }
                )),
        ),
        panes[0],
    );
    let columns = Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(panes[1]);
    let labels = viewer
        .records
        .iter()
        .map(|r| {
            let kind = text(r, "type");
            let indent = if kind.eq_ignore_ascii_case("task") {
                "    "
            } else if kind.eq_ignore_ascii_case("job") {
                "  "
            } else {
                ""
            };
            let progress = r["percentComplete"]
                .as_i64()
                .map(|n| format!(" {n}%"))
                .unwrap_or_default();
            format!("{indent}{} {}{progress}", run_icon(r), text(r, "name"))
                .chars()
                .skip(horizontal[0] as usize)
                .collect::<String>()
        })
        .collect();
    let mut selected = ListState::default().with_selected(Some(viewer.selection()));
    draw_list(
        frame,
        columns[0],
        if focus == 0 {
            "▶ Stage / job / task"
        } else {
            "Stage / job / task"
        },
        labels,
        &mut selected,
        0,
    );
    let offset = viewer.offset(columns[1].height.saturating_sub(2) as usize);
    let log = if viewer.visible_log().lines.is_empty() {
        if !viewer.error.is_empty() {
            viewer.error.clone()
        } else if viewer
            .records
            .get(viewer.selection())
            .is_some_and(|r| text(r, "state") == "pending")
        {
            "This task has not started.".into()
        } else {
            format!(
                "{}\nWaiting for console output or an uploaded log. Polling continues every 2 seconds.",
                viewer.live_status
            )
        }
    } else {
        viewer
            .visible_log()
            .lines
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    };
    let task_timer = viewer
        .records
        .get(viewer.selection())
        .map(|record| run_timer(record, now))
        .unwrap_or_else(|| "Not started".into());
    let title = format!(
        "Task logs | {task_timer} | {} | {} | retained from line {}",
        if viewer.showing_live() {
            "live console tail"
        } else {
            "uploaded log"
        },
        if viewer.follow {
            "following"
        } else {
            "scroll paused"
        },
        viewer.visible_log().base
    );
    frame.render_widget(
        Paragraph::new(log).scroll((offset, horizontal[1])).block(
            Block::bordered()
                .border_style(if focus == 1 {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                })
                .title(title)
                .title_bottom(output::clean(&viewer.live_status)),
        ),
        columns[1],
    );
}
fn center(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - height) / 2),
        Constraint::Percentage(height),
        Constraint::Percentage((100 - height) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - width) / 2),
        Constraint::Percentage(width),
        Constraint::Percentage((100 - width) / 2),
    ])
    .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn timers_tick_and_freeze_when_finished() -> Result<()> {
        let start = time::OffsetDateTime::parse(
            "2026-10-29T09:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )?;
        let running = json!({"startTime":"2026-10-29T09:00:00Z", "status":"inProgress"});
        assert_eq!(
            run_timer(&running, start + time::Duration::seconds(3661)),
            "01:01:01"
        );
        assert_eq!(
            run_timer(&running, start + time::Duration::seconds(3662)),
            "01:01:02"
        );
        let completed = json!({"startTime":"2026-10-29T09:00:00Z", "finishTime":"2026-10-29T09:02:03Z", "status":"completed"});
        assert_eq!(
            run_timer(&completed, start + time::Duration::hours(5)),
            "00:02:03"
        );
        assert_eq!(run_timer(&json!({"state":"pending"}), start), "Not started");
        assert_eq!(run_timer(&json!({"startTime":"bad"}), start), "Not started");
        assert_eq!(
            run_timer(
                &json!({"startTime":"2026-10-29T09:00:00Z", "state":"completed"}),
                start
            ),
            "Duration unavailable"
        );
        assert_eq!(
            run_timer(&running, start - time::Duration::seconds(1)),
            "00:00:00"
        );
        Ok(())
    }

    #[test]
    fn readable_run_times_and_pipeline_api_durations() -> Result<()> {
        let now = time::OffsetDateTime::parse(
            "2026-10-29T09:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )?;
        assert_eq!(
            human_timestamp("2026-10-29T10:59:40+02:00", now),
            "Just now"
        );
        assert_eq!(human_timestamp("2026-10-29T08:58:00Z", now), "2min ago");
        assert_eq!(human_timestamp("2026-10-29T07:00:00Z", now), "2h ago");
        assert_eq!(human_timestamp("2026-10-28T09:00:00Z", now), "1d ago");
        assert_eq!(human_timestamp("2026-10-29T09:02:00Z", now), "In 2min");
        assert_eq!(
            human_timestamp("2026-10-01T11:00:00+02:00", now),
            "1 Oct 2026 09:00 UTC"
        );
        assert_eq!(human_timestamp("", now), "Unknown");
        assert_eq!(
            run_timer(
                &json!({"createdDate":"2026-10-29T08:00:00Z", "finishedDate":"2026-10-29T08:02:10Z", "state":"completed"}),
                now
            ),
            "00:02:10"
        );
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Recent(vec![
            json!({"id":1,"definition":{"name":"Build app"},"queueTime":"2026-10-01T09:00:00Z","startTime":"2026-10-01T09:00:00Z","finishTime":"2026-10-01T09:02:10Z","status":"completed","result":"succeeded"}),
        ]);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35))?;
        terminal.draw(|frame| draw(frame, &mut app))?;
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Duration"));
        assert!(rendered.contains("00:02:10"));
        assert!(!rendered.contains("2026-10-01T"));
        Ok(())
    }

    fn fixture() -> Result<(tempfile::TempDir, App)> {
        let root = tempfile::tempdir()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let app = App::new(
            Store {
                root: root.path().join("config"),
            },
            Settings {
                organization: Some("saved".into()),
                project: Some("project".into()),
                ..Default::default()
            },
            None,
            None,
            tx,
        );
        Ok((root, app))
    }
    fn key(code: KeyCode) -> Event {
        Event::Key(event::KeyEvent::new(code, KeyModifiers::NONE))
    }
    #[tokio::test]
    async fn saved_project_opens_workspace_without_setup_on_restart() -> Result<()> {
        let (_root, mut app) = fixture()?;
        auth::pat::save(&app.store, "saved", "fixture-pat")?;
        app.setup_org = Some("saved".into());
        app.screen = Screen::Projects(vec![json!({"id":"chosen-id", "name":"Chosen Project"})]);
        app.enter()?;
        let settings = app.store.load()?;
        assert_eq!(settings.project.as_deref(), Some("Chosen Project"));
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut restarted = App::new(app.store.clone(), settings, None, None, tx);
        restarted.start()?;
        assert!(restarted.form.is_none());
        assert!(matches!(restarted.screen, Screen::Prs(_)));
        assert!(restarted.setup_org.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn tabs_switch_during_reads_and_from_logs_but_not_writes() -> Result<()> {
        let (_root, mut app) = fixture()?;
        auth::pat::save(&app.store, "saved", "fixture-pat")?;
        app.load_prs()?;
        let old_generation = app.generation;
        app.handle(key(KeyCode::Tab))?;
        assert!(matches!(app.screen, Screen::Recent(_)));
        app.receive((old_generation, Message::Prs(vec![])));
        assert!(matches!(app.screen, Screen::Recent(_)));
        app.handle(key(KeyCode::BackTab))?;
        assert!(matches!(app.screen, Screen::Prs(_)));
        app.writing = true;
        let generation = app.generation;
        app.handle(key(KeyCode::Tab))?;
        assert_eq!(app.generation, generation);
        app.cancel_task();
        app.screen = Screen::Watch(Box::new(Viewer::new(1)));
        app.handle(key(KeyCode::Tab))?;
        assert!(matches!(app.screen, Screen::Prs(_)));
        Ok(())
    }

    #[test]
    fn activity_ignores_another_pipeline_and_renders_current_runs() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Pipelines(vec![json!({"id":7,"name":"Build app"})]);
        app.activity_pipeline = Some(7);
        app.receive((
            app.generation,
            Message::Activity {
                pipeline: 8,
                values: vec![json!({"id":99})],
            },
        ));
        assert!(app.activity.is_empty());
        app.receive((
            app.generation,
            Message::Activity {
                pipeline: 7,
                values: vec![
                    json!({"id":42,"name":"Build 42","state":"completed","result":"succeeded"}),
                ],
            },
        ));
        for width in [60, 120] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 35))?;
            terminal.draw(|f| draw(f, &mut app))?;
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(rendered.contains("Activity | Build app"));
            assert!(rendered.contains("succeeded"));
        }
        Ok(())
    }

    #[test]
    fn cached_tab_navigation_preserves_selection_without_network() -> Result<()> {
        let (_root, mut app) = fixture()?;
        let target = app.settings.target(None, None)?;
        app.pr_cache = Some((
            target.clone(),
            false,
            vec![json!({"pullRequestId":1}), json!({"pullRequestId":2})],
        ));
        app.pipeline_cache = Some((target, vec![json!({"id":7})]));
        app.activity_cache
            .insert(app.activity_key(7)?, vec![json!({"id":42})]);
        app.load_prs()?;
        app.selection.select(Some(1));
        app.load_all_pipelines()?;
        assert!(matches!(app.screen, Screen::Pipelines(_)));
        assert_eq!(app.activity.len(), 1);
        assert!(app.task.is_none());
        app.load_prs()?;
        assert_eq!(app.selection.selected(), Some(1));
        assert!(app.task.is_none());
        // Another project must never reuse these cached results.
        app.settings.project = Some("different".into());
        assert!(app.load_prs().is_err());
        Ok(())
    }

    #[test]
    fn settings_groups_project_and_account_without_changing_main_navigation() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Prs(vec![]);
        app.handle(key(KeyCode::Char('s')))?;
        assert_eq!(app.settings_menu, Some(0));
        app.handle(key(KeyCode::Tab))?;
        assert_eq!(app.settings_menu, Some(1));
        assert!(matches!(app.screen, Screen::Prs(_)));
        app.handle(key(KeyCode::Enter))?;
        assert!(app.settings_menu.is_none());
        assert!(app.form.is_some());
        app.handle(key(KeyCode::Esc))?;
        app.handle(key(KeyCode::Char('s')))?;
        app.handle(key(KeyCode::Esc))?;
        assert!(app.settings_menu.is_none());
        assert!(matches!(app.screen, Screen::Prs(_)));
        Ok(())
    }

    #[test]
    fn recent_refresh_keeps_selected_run_when_new_runs_arrive() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Recent(vec![json!({"id":42}), json!({"id":41})]);
        app.selection.select(Some(1));
        app.receive((
            app.generation,
            Message::Recent(vec![json!({"id":43}), json!({"id":42}), json!({"id":41})]),
        ));
        assert_eq!(app.selection.selected(), Some(2));
        app.handle(key(KeyCode::Up))?;
        assert_eq!(app.selection.selected(), Some(1));
        app.enter()?;
        assert!(matches!(&app.screen, Screen::Watch(viewer) if viewer.run_id == 42));
        Ok(())
    }

    #[test]
    fn pipeline_table_groups_folders_and_keeps_pipeline_rows_selectable() -> Result<()> {
        let (_root, mut app) = fixture()?;
        let mut items = vec![
            json!({"id":3,"name":"Z","folder":"B"}),
            json!({"id":2,"name":"Beta","folder":"A"}),
            json!({"id":1,"name":"Alpha","folder":"A"}),
        ];
        sort_pipelines(&mut items);
        assert_eq!(
            items.iter().map(|v| v["id"].as_i64()).collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
        assert_eq!(pipeline_folder(&json!({"folder":""})), "Root");
        app.screen = Screen::Pipelines(items);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35))?;
        terminal.draw(|f| draw(f, &mut app))?;
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(rendered.contains("Pipelines by folder"));
        assert!(rendered.contains("Alpha"));
        app.handle(key(KeyCode::Down))?;
        assert_eq!(app.selection.selected(), Some(1));
        Ok(())
    }

    #[tokio::test]
    async fn background_log_refresh_does_not_flash_loading_and_leaving_stops_stream() -> Result<()>
    {
        let (_root, mut app) = fixture()?;
        auth::pat::save(&app.store, "saved", "fixture-pat")?;
        let mut viewer = Viewer::new(42);
        viewer.run = json!({"id":42,"status":"inProgress"});
        app.screen = Screen::Watch(Box::new(viewer));
        app.status = "Ready".into();
        app.poll()?;
        assert!(app.loading_since.is_none());
        assert_eq!(app.status, "Ready");
        app.cancel_task();
        let (tx, rx) = mpsc::channel(1);
        app.live = Some(LiveSession {
            task: tokio::spawn(async move {
                tx.closed().await;
            }),
            rx,
        });
        app.set_screen(Screen::Home);
        assert!(app.live.is_none());
        app.screen = Screen::Watch(Box::new(Viewer::new(42)));
        app.poll()?;
        assert!(app.loading_since.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn loading_animates_and_refresh_keeps_rows_until_cancelled() -> Result<()> {
        let (_root, mut app) = fixture()?;
        auth::pat::save(&app.store, "saved", "fixture-pat")?;
        app.screen = Screen::Prs(vec![json!({"pullRequestId":42,"title":"Existing PR"})]);
        app.load_prs()?;
        assert!(matches!(&app.screen, Screen::Prs(rows) if rows.len() == 1));
        assert!(app.loading_since.is_some());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35))?;
        terminal.draw(|frame| draw(frame, &mut app))?;
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Existing PR"));
        assert!(rendered.contains("Loading pull requests"));
        assert_ne!(
            loading_text("Loading", Duration::ZERO, false),
            loading_text("Loading", Duration::from_millis(100), false)
        );
        app.handle(key(KeyCode::Esc))?;
        assert!(app.task.is_none());
        assert!(app.loading_since.is_none());
        assert!(matches!(&app.screen, Screen::Prs(rows) if rows.len() == 1));
        assert!(!loading_text("Saving", Duration::ZERO, true).contains("cancel"));
        Ok(())
    }

    #[test]
    fn focused_log_pane_scrolls_without_changing_task_and_icons_cover_results() -> Result<()> {
        let (_root, mut app) = fixture()?;
        let mut viewer = Viewer::new(1);
        viewer
            .log
            .append(0, (0..100).map(|n| format!("line {n}")).collect());
        app.screen = Screen::Watch(Box::new(viewer));
        app.handle(key(KeyCode::F(6)))?;
        assert_eq!(app.pane_focus, 1);
        app.handle(key(KeyCode::PageDown))?;
        app.handle(key(KeyCode::Right))?;
        let Screen::Watch(viewer) = &app.screen else {
            anyhow::bail!("Expected watch");
        };
        assert_eq!(viewer.top, 20);
        assert!(!viewer.follow);
        assert_eq!(app.pane_x[1], 8);
        assert_eq!(run_icon(&json!({"result":"succeeded"})), "✓");
        assert_eq!(run_icon(&json!({"result":"failed"})), "✗");
        assert_eq!(run_icon(&json!({"result":"skipped"})), "−");
        Ok(())
    }

    #[test]
    fn project_picker_highlights_current_project() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.setup_org = app.settings.organization.clone();
        app.receive((
            app.generation,
            Message::Projects(vec![json!({"name":"another"}), json!({"name":"project"})]),
        ));
        assert_eq!(app.selection.selected(), Some(1));
        Ok(())
    }

    #[test]
    fn pat_setup_asks_for_organization_and_a_masked_token_not_app_registration() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.settings.auth = AuthMethod::Pat;
        app.identity_form();
        let form = app.form.as_ref().context("Expected setup form")?;
        assert!(matches!(form.kind, FormKind::PatIdentity));
        assert_eq!(form.fields.len(), 2);
        assert_eq!(form.value(0)?, "saved");
        assert!(form.fields[0].name.contains("Organization"));
        assert!(form.fields[1].secret);
        assert!(form.fields[1].value.is_empty());
        Ok(())
    }

    #[test]
    fn pat_form_never_renders_entered_or_pasted_token_text() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.identity_form();
        app.handle(key(KeyCode::Tab))?;
        app.handle(Event::Paste("fixture-secret-pat".into()))?;
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend)?;
        terminal.draw(|f| draw(f, &mut app))?;
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(rendered.contains("******************"));
        assert!(!rendered.contains("fixture-secret-pat"));
        app.handle(key(KeyCode::Esc))?;
        assert!(app.form.is_none());
        assert!(!app.store.root.exists());
        Ok(())
    }
    #[test]
    fn pat_setup_commits_the_target_only_when_a_project_is_selected() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.settings.auth = AuthMethod::Pat;
        app.store.save(&app.settings)?;
        let before = app.settings.clone();
        app.setup_org = Some("different".into());
        app.screen = Screen::Projects(vec![]);
        app.handle(key(KeyCode::Esc))?;
        assert_eq!(app.store.load()?, before);

        app.screen = Screen::Projects(vec![json!({"id":"project-id","name":"New Project"})]);
        app.org = Some("override".into());
        app.project = Some("override-project".into());
        app.enter()?;
        let saved = app.store.load()?;
        assert_eq!(saved.auth, AuthMethod::Pat);
        assert_eq!(saved.organization.as_deref(), Some("different"));
        assert_eq!(saved.project.as_deref(), Some("New Project"));
        assert!(app.org.is_none() && app.project.is_none());
        assert!(saved.tenant_id.is_none() && saved.client_id.is_none());
        Ok(())
    }
    #[test]
    fn pr_detail_tabs_render_structured_content_at_both_widths() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Pr {
            value: json!({
                "pullRequestId": 7, "title": "Example PR", "description": "Description text",
                "reviewers": [{"displayName":"Sam", "vote":10}],
                "detailThreads": [{"comments":[{"author":{"displayName":"Alex"}, "content":"Visible comment"}, {"isDeleted":true,"content":"Deleted comment"}]}]
            }),
        };
        app.handle(key(KeyCode::Char(']')))?;
        assert_eq!(app.pr_tab, 1);
        for width in [70, 120] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30))?;
            let Screen::Pr { value } = &app.screen else {
                anyhow::bail!("Expected PR detail screen")
            };
            terminal.draw(|frame| render_pr(frame, frame.area(), value, app.pr_tab, 0, [0; 3]))?;
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(rendered.contains("Visible comment"));
            assert!(!rendered.contains("Deleted comment"));
            if width == 120 {
                assert!(rendered.contains("Approved"));
            }
        }
        app.handle(key(KeyCode::Char('v')))?;
        assert!(app.form.is_none());
        Ok(())
    }

    #[test]
    fn abandonment_requires_a_separate_confirmation() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Pr {
            value: json!({"pullRequestId":7}),
        };
        app.open_pr_form('a')?;
        assert!(matches!(
            app.confirm,
            Some(PrCommand::Abandon { id: 7, yes: true })
        ));
        assert!(app.task.is_none());
        app.handle(key(KeyCode::Char('n')))?;
        assert!(app.confirm.is_none());
        assert!(app.task.is_none());
        Ok(())
    }
    #[test]
    fn pipeline_screen_has_no_write_controls() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Watch(Box::new(Viewer::new(9)));
        for c in ['m', 'a', 'r', 'n', 'c'] {
            app.handle(key(KeyCode::Char(c)))?;
        }
        assert!(app.confirm.is_none());
        assert!(app.form.is_none());
        assert!(app.task.is_none());
        Ok(())
    }
    #[test]
    fn unavailable_log_does_not_discard_run_progress() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Watch(Box::new(Viewer::new(9)));
        app.receive((
            app.generation,
            Message::Snapshot {
                run: json!({"id":9,"status":"inProgress"}),
                records: vec![],
                log: None,
                log_error: Some("Log not available yet".into()),
            },
        ));
        let Screen::Watch(viewer) = &app.screen else {
            anyhow::bail!("Expected viewer screen");
        };
        assert_eq!(viewer.run["status"], "inProgress");
        assert!(app.status.contains("Log not available"));
        Ok(())
    }
    #[test]
    fn canceling_setup_does_not_switch_saved_organization() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.setup_org = Some("different".into());
        app.screen = Screen::Projects(vec![]);
        app.handle(key(KeyCode::Esc))?;
        assert_eq!(app.settings.organization.as_deref(), Some("saved"));
        assert_eq!(app.settings.project.as_deref(), Some("project"));
        Ok(())
    }
    #[test]
    fn renders_task_sidebar_and_main_log_pane() -> Result<()> {
        let (_root, mut app) = fixture()?;
        let mut viewer = Viewer::new(9);
        viewer.update(json!({"id":9,"status":"inProgress"}),vec![json!({"id":"job","name":"Build job","type":"Job","state":"inProgress"}),json!({"id":"task","parentId":"job","name":"Compile","type":"Task","state":"inProgress","log":{"id":1}})]);
        viewer.accept_log(
            viewer.log_key.clone().context("Expected task log")?,
            0,
            vec!["compiler output".into()],
        );
        app.screen = Screen::Watch(Box::new(viewer));
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend)?;
        terminal.draw(|f| draw(f, &mut app))?;
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        for expected in [
            "Stage / job / task",
            "Compile",
            "compiler output",
            "Following",
        ] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
        Ok(())
    }

    #[test]
    fn stale_replies_cannot_change_a_new_screen_or_clear_status() -> Result<()> {
        let (_root, mut app) = fixture()?;
        let old_generation = app.generation;
        app.cancel_task();
        app.screen = Screen::Watch(Box::new(Viewer::new(10)));
        app.status = "New request pending".into();
        for message in [
            Message::Snapshot {
                run: json!({"id":9}),
                records: vec![],
                log: None,
                log_error: None,
            },
            Message::Prs(vec![]),
            Message::Error("Old error".into()),
            Message::LoginProgress("Old code".into()),
        ] {
            app.receive((old_generation, message));
        }
        let Screen::Watch(viewer) = &app.screen else {
            anyhow::bail!("Stale reply changed the screen");
        };
        assert_eq!(viewer.run_id, 10);
        assert!(viewer.run.is_null());
        assert_eq!(app.status, "New request pending");
        Ok(())
    }

    #[test]
    fn incomplete_forms_return_errors_instead_of_panicking() -> Result<()> {
        let form = Form::new(FormKind::Create, vec![]);
        assert!(form.command().is_err());
        let (_root, mut app) = fixture()?;
        app.form = Some(form);
        assert!(app.handle(key(KeyCode::Tab)).is_err());
        assert!(app.handle(Event::Paste("text".into())).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn stale_reply_cannot_release_a_new_inflight_task() -> Result<()> {
        let (_root, mut app) = fixture()?;
        let old_generation = app.generation;
        app.launch(std::future::pending());
        app.receive((old_generation, Message::Error("Old error".into())));
        assert!(app.task.is_some());
        assert_eq!(app.status, "Loading...");
        Ok(())
    }
}
