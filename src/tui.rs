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
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
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
    Pr { value: Value, detail: String },
    Pipelines(Vec<Value>),
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
        detail: String,
    },
    Pipelines(Vec<Value>),
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
    Written(String),
    Error(String),
}

enum FormKind {
    Identity,
    PatIdentity,
    Create,
    Edit(i32),
    Comment(i32),
    Review(i32),
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
            FormKind::Review(id) => PrCommand::Review {
                id,
                vote: Vote::from_str(self.value(0)?, true).map_err(|e| anyhow::anyhow!(e))?,
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
    form: Option<Form>,
    confirm: Option<PrCommand>,
    status: String,
    task: Option<JoinHandle<()>>,
    tx: mpsc::UnboundedSender<(u64, Message)>,
    generation: u64,
    poll_at: Instant,
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
            form: None,
            confirm: None,
            status: String::new(),
            task: None,
            tx,
            generation: 0,
            poll_at: Instant::now(),
        }
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
        self.task = Some(tokio::spawn(async move {
            let message = match future.await {
                Ok(message) => message,
                Err(e) => Message::Error(output::clean(&e.to_string())),
            };
            let _ = tx.send((generation, message));
        }));
    }
    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.selection = ListState::default().with_selected(Some(0));
        self.detail_scroll = 0;
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
        let api = self.api()?;
        let attention = self.attention;
        self.launch(async move {
            Ok(Message::Prs(if attention {
                api.attention_prs().await?
            } else {
                api.pr_list(PrState::Active, None, None, None).await?
            }))
        });
        Ok(())
    }
    fn load_pipelines(&mut self) -> Result<()> {
        let api = self.api()?;
        self.launch(async move { Ok(Message::Pipelines(api.pipeline_list().await?)) });
        Ok(())
    }
    fn load_pr(&mut self, id: i32) -> Result<()> {
        let api = self.api()?;
        self.launch(async move {
            let (value, detail) = api.pr_details(id).await?;
            Ok(Message::Pr { value, detail })
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
        Ok(())
    }
    fn poll(&mut self) -> Result<()> {
        let Screen::Watch(viewer) = &self.screen else {
            return Ok(());
        };
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
        self.status = String::new();
        match message {
            Message::Organizations(items) => self.set_screen(Screen::Organizations(items)),
            Message::Projects(items) => self.set_screen(Screen::Projects(items)),
            Message::Prs(items) => self.set_screen(Screen::Prs(items)),
            Message::Pr { value, detail } => self.set_screen(Screen::Pr { value, detail }),
            Message::Pipelines(items) => self.set_screen(Screen::Pipelines(items)),
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
                if settings.auth == AuthMethod::Pat {
                    self.org = None;
                    self.project = None;
                }
                self.settings = settings;
                self.setup_org = None;
                self.set_screen(Screen::Home);
                self.status = "Saved defaults. p: pull requests, l: pipelines".into();
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
            Screen::Runs { values, .. } => {
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
            'v' => {
                self.form = Some(Form::new(
                    FormKind::Review(id),
                    vec![(
                        "approve / approve-with-suggestions / reset / wait / reject",
                        "approve".into(),
                    )],
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
        match &self.screen {
            Screen::Prs(_) => self.load_prs()?,
            Screen::Pr { value, .. } => self.load_pr(response_id(value, "pullRequestId")?)?,
            Screen::Pipelines(_) => self.load_pipelines()?,
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
        if key.code == KeyCode::Char('q') {
            return Ok(true);
        }
        // Keep navigation responsive while polling, but don't overlap writes or setup.
        if self.task.is_some() && !matches!(self.screen, Screen::Watch(_)) {
            return Ok(false);
        }
        if let Screen::Watch(viewer) = &mut self.screen {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => viewer.navigate(-1),
                KeyCode::Down | KeyCode::Char('j') => viewer.navigate(1),
                KeyCode::PageUp => viewer.scroll(-20),
                KeyCode::PageDown => viewer.scroll(20),
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
        match key.code {
            KeyCode::Esc => self.set_screen(Screen::Home),
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
            KeyCode::Char('s') => self.identity_form(),
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
                    | Screen::Pipelines(v) => v.len(),
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
    if auth::Credential::new(app.store.clone(), app.settings.clone()).is_err()
        || app
            .settings
            .target(app.org.as_deref(), app.project.as_deref())
            .is_err()
    {
        app.identity_form();
    }
    let (_guard, mut terminal) = TerminalGuard::enter()?;
    loop {
        while let Ok(message) = rx.try_recv() {
            app.receive(message);
        }
        if app.task.is_none()
            && matches!(app.screen, Screen::Watch(_))
            && Instant::now() >= app.poll_at
            && let Err(error) = app.poll()
        {
            app.status = error.to_string();
            app.poll_at = Instant::now() + Duration::from_secs(2);
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

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let layout = Layout::vertical([
        Constraint::Length(2),
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
        Paragraph::new(output::cell(&format!("DevOps   {context}")))
            .style(Style::default().fg(Color::Cyan)),
        layout[0],
    );
    let help = match &mut app.screen {
        Screen::Home => {
            frame.render_widget(Paragraph::new("p  Pull requests\ni  Assigned reviews needing attention\nl  YAML pipelines\ns  Sign in / setup\nq  Quit\n\nThe pipeline screen is observation-only. Use the CLI for pipeline writes.\n\nSetup asks for a personal access token (hidden input); no app registration required.\nRepository autodetection is not available yet; choose your organization and project.\nExisting Windows and WSL sessions are separate.").block(Block::bordered().title("Home")),layout[1]);
            "p: PRs   i: assigned reviews   l: pipelines   s: setup   q: quit"
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
            );
            "Up/Down: select   Enter: continue   Esc: home"
        }
        Screen::Projects(items) => {
            draw_list(
                frame,
                layout[1],
                "Select default project",
                items.iter().map(|v| text(v, "name").into()).collect(),
                &mut app.selection,
            );
            "Up/Down: select   Enter: save   Esc: home"
        }
        Screen::Prs(items) => {
            draw_list(
                frame,
                layout[1],
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
            );
            "Enter: view   n: create   i: toggle assigned reviews   F5: refresh   Esc: home"
        }
        Screen::Pr { detail, .. } => {
            frame.render_widget(
                Paragraph::new(detail.as_str())
                    .wrap(Wrap { trim: false })
                    .scroll((app.detail_scroll, 0))
                    .block(Block::bordered().title("Pull request")),
                layout[1],
            );
            "n: create   e: edit   c: comment   v: vote   m: merge   a: abandon   r: reactivate\nPgUp/PgDn: scroll   F5: refresh   Esc: home"
        }
        Screen::Pipelines(items) => {
            draw_list(
                frame,
                layout[1],
                "YAML pipelines",
                items
                    .iter()
                    .map(|v| format!("{}  {}", scalar(&v["id"]), text(v, "name")))
                    .collect(),
                &mut app.selection,
            );
            "Enter: runs   F5: refresh   Esc: home"
        }
        Screen::Runs { values, .. } => {
            draw_list(
                frame,
                layout[1],
                "Recent runs",
                values
                    .iter()
                    .map(|v| {
                        format!(
                            "{}  {}  {}  {}",
                            scalar(&v["id"]),
                            text(v, "name"),
                            text(v, "state"),
                            text(v, "result")
                        )
                    })
                    .collect(),
                &mut app.selection,
            );
            "Enter: observe   F5: refresh   Esc: home"
        }
        Screen::Watch(viewer) => {
            draw_viewer(frame, layout[1], viewer);
            "Up/Down: select task/job   PgUp/PgDn: scroll logs   f: resume following   Esc: home"
        }
    };
    frame.render_widget(
        Paragraph::new(output::clean(&format!("{help}\n{}", app.status)))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::TOP)),
        layout[2],
    );
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

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<String>,
    state: &mut ListState,
) {
    let items = if items.is_empty() {
        vec![ListItem::new("No items")]
    } else {
        items
            .into_iter()
            .map(|s| ListItem::new(output::cell(&s)))
            .collect()
    };
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("> ")
            .highlight_style(Style::default().fg(Color::Yellow))
            .block(Block::bordered().title(title)),
        area,
        state,
    );
}
fn draw_viewer(frame: &mut Frame, area: Rect, viewer: &mut Viewer) {
    let panes = Layout::vertical([Constraint::Length(8), Constraint::Min(1)]).split(area);
    let elapsed = viewer.run["startTime"]
        .as_str()
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .map(|start| {
            let end = viewer.run["finishTime"]
                .as_str()
                .and_then(|s| {
                    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_else(time::OffsetDateTime::now_utc);
            format!("{}s", (end - start).whole_seconds().max(0))
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(output::clean(&format!(
            "{}\nElapsed: {elapsed}",
            build_summary(&viewer.run)
        )))
        .block(Block::bordered().title(if viewer.follow {
            "Following"
        } else {
            "Paused"
        })),
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
            let state = if text(r, "result").is_empty() {
                text(r, "state")
            } else {
                text(r, "result")
            };
            format!(
                "{indent}[{state}] {} {}%",
                text(r, "name"),
                scalar(&r["percentComplete"])
            )
        })
        .collect();
    let mut selected = ListState::default().with_selected(Some(viewer.selection()));
    draw_list(
        frame,
        columns[0],
        "Stage / job / task",
        labels,
        &mut selected,
    );
    let offset = viewer.offset(columns[1].height.saturating_sub(2) as usize);
    let log = if viewer.log.lines.is_empty() {
        "Waiting for task logs...".into()
    } else {
        viewer
            .log
            .lines
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    };
    let title = format!(
        "Task logs | {} | retained from line {}",
        if viewer.follow { "live" } else { "paused" },
        viewer.log.base
    );
    frame.render_widget(
        Paragraph::new(log)
            .scroll((offset, 0))
            .block(Block::bordered().title(title)),
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
    fn abandonment_requires_a_separate_confirmation() -> Result<()> {
        let (_root, mut app) = fixture()?;
        app.screen = Screen::Pr {
            value: json!({"pullRequestId":7}),
            detail: String::new(),
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
