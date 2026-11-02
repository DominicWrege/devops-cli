use super::*;
use crate::service::{TagChange, commit_date, ref_commit, tag_name};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(test)]
mod tests;

mod tags;
pub(super) use tags::{TagForm, TagWrite};

#[derive(Default)]
pub(super) struct Filter {
    pub query: String,
    pub editing: bool,
    target: Option<Target>,
}

#[derive(Default)]
pub(super) struct History {
    pub commits: Vec<Value>,
    pub selection: ListState,
    pub more: bool,
}

pub(super) struct View {
    pub tab: usize,
    pub branch: String,
    pub histories: HashMap<String, History>,
    pub tags: Option<Vec<Value>>,
    pub branches: Option<Vec<Value>>,
    pub tags_selection: ListState,
    pub branch_selection: ListState,
    pub collapsed_tags: BTreeSet<String>,
    pub collapsed_branches: BTreeSet<String>,
    pub picking_branch: bool,
    pub detail: Option<Value>,
}

impl View {
    pub fn new(repo: &Value) -> Self {
        Self {
            tab: 0,
            branch: text(repo, "defaultBranch")
                .trim_start_matches("refs/heads/")
                .into(),
            histories: HashMap::new(),
            tags: None,
            branches: None,
            tags_selection: ListState::default(),
            branch_selection: ListState::default(),
            collapsed_tags: BTreeSet::new(),
            collapsed_branches: BTreeSet::new(),
            picking_branch: false,
            detail: None,
        }
    }

    fn refs(&self) -> &[Value] {
        if self.picking_branch {
            self.branches.as_deref()
        } else {
            self.tags.as_deref()
        }
        .unwrap_or_default()
    }

    fn rows(&self) -> Vec<RefRow> {
        ref_rows(
            self.refs(),
            self.picking_branch,
            if self.picking_branch {
                &self.collapsed_branches
            } else {
                &self.collapsed_tags
            },
        )
    }

    fn ref_selection(&mut self) -> &mut ListState {
        if self.picking_branch {
            &mut self.branch_selection
        } else {
            &mut self.tags_selection
        }
    }

    pub(super) fn selected_ref(&self) -> Option<usize> {
        let state = if self.picking_branch {
            &self.branch_selection
        } else {
            &self.tags_selection
        };
        self.rows().get(state.selected().unwrap_or(0))?.index
    }

    fn selected_commit(&self) -> Option<&str> {
        if let Some(detail) = &self.detail {
            return detail["detailChanges"]["head"].as_str();
        }
        match self.tab {
            0 => {
                let history = self.histories.get(&self.branch)?;
                history
                    .commits
                    .get(history.selection.selected().unwrap_or(0))?["commitId"]
                    .as_str()
            }
            2 => Some(ref_commit(self.tags.as_ref()?.get(self.selected_ref()?)?)),
            _ => None,
        }
    }
}

pub(super) enum Reply {
    TagWritten {
        change: TagChange,
        refreshed: Result<Vec<Value>, String>,
    },
    Commits {
        branch: String,
        skip: usize,
        values: Vec<Value>,
    },
    Refs {
        branches: bool,
        values: Vec<Value>,
    },
    Changes(Value),
    Diff {
        commit: String,
        index: usize,
        diff: String,
    },
}

#[derive(Debug)]
struct RefRow {
    path: String,
    name: String,
    depth: usize,
    index: Option<usize>,
    count: usize,
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    entries: Vec<usize>,
}
impl Node {
    fn count(&self) -> usize {
        self.entries.len() + self.children.values().map(Node::count).sum::<usize>()
    }
    fn flatten(
        &self,
        path: &str,
        depth: usize,
        values: &[Value],
        collapsed: &BTreeSet<String>,
        branches: bool,
        rows: &mut Vec<RefRow>,
    ) {
        for (name, node) in &self.children {
            let child_path = format!("{path}{name}/");
            rows.push(RefRow {
                path: child_path.clone(),
                name: format!("{name}/"),
                depth,
                index: None,
                count: node.count(),
            });
            if !collapsed.contains(&child_path) {
                node.flatten(&child_path, depth + 1, values, collapsed, branches, rows);
            }
        }
        let mut entries = self.entries.clone();
        entries.sort_by(|a, b| {
            let a = &values[*a];
            let b = &values[*b];
            let order = if branches {
                std::cmp::Ordering::Equal
            } else {
                commit_date(&b["targetCommit"]).cmp(&commit_date(&a["targetCommit"]))
            };
            order.then_with(|| text(a, "name").cmp(text(b, "name")))
        });
        let root_group = path.is_empty() && !self.children.is_empty() && !entries.is_empty();
        if root_group {
            rows.push(RefRow {
                path: String::new(),
                name: "(root)".into(),
                depth,
                index: None,
                count: entries.len(),
            });
            if collapsed.contains("") {
                return;
            }
        }
        for index in entries {
            rows.push(RefRow {
                path: text(&values[index], "name").into(),
                name: text(&values[index], "name")
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .into(),
                depth: depth + usize::from(root_group),
                index: Some(index),
                count: 0,
            });
        }
    }
}

fn ref_rows(values: &[Value], branches: bool, collapsed: &BTreeSet<String>) -> Vec<RefRow> {
    let prefix = if branches {
        "refs/heads/"
    } else {
        "refs/tags/"
    };
    let mut root = Node::default();
    for (index, value) in values.iter().enumerate() {
        let Some(name) = text(value, "name").strip_prefix(prefix) else {
            continue;
        };
        let mut parts: Vec<_> = name.split('/').collect();
        parts.pop();
        let mut node = &mut root;
        for part in parts {
            node = node.children.entry(part.into()).or_default();
        }
        node.entries.push(index);
    }
    let mut rows = Vec::new();
    root.flatten("", 0, values, collapsed, branches, &mut rows);
    rows
}

impl App {
    pub(super) fn scope_repository_filter(&mut self, target: &Target) {
        if self.repo_filter.target.as_ref() != Some(target) {
            self.repo_filter = Filter {
                target: Some(target.clone()),
                ..Filter::default()
            };
        }
    }

    pub(super) fn show_repositories(&mut self, mut items: Vec<Value>) {
        let selected_id = if let Screen::Repos(previous) = &self.screen {
            previous
                .get(self.selection.selected().unwrap_or(0))
                .and_then(|repo| repo["id"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
        let query = self.repo_filter.query.to_lowercase();
        items.retain(|repo| text(repo, "name").to_lowercase().contains(&query));
        let selected = selected_id.map(|id| {
            items
                .iter()
                .position(|repo| repo["id"].as_str() == Some(&id))
                .unwrap_or(0)
        });
        self.set_screen(Screen::Repos(items));
        if let Some(selected) = selected {
            self.selection.select(Some(selected));
        }
    }

    pub(super) fn apply_repository_filter(&mut self) -> Result<()> {
        let target = self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())?;
        if let Some((cached, items)) = &self.repo_cache
            && *cached == target
        {
            self.show_repositories(items.clone());
        }
        Ok(())
    }

    pub(super) fn handle_repository_filter(&mut self, key: event::KeyEvent) -> Result<bool> {
        let Screen::Repos(items) = &self.screen else {
            return Ok(false);
        };
        if self.repo_filter.editing {
            match key.code {
                KeyCode::Enter => self.repo_filter.editing = false,
                KeyCode::Esc => {
                    self.repo_filter.editing = false;
                    self.repo_filter.query.clear();
                    self.apply_repository_filter()?;
                }
                KeyCode::Backspace => {
                    self.repo_filter.query.pop();
                    self.apply_repository_filter()?;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.repo_filter.query.clear();
                    self.apply_repository_filter()?;
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.repo_filter.query.push(c);
                    self.apply_repository_filter()?;
                }
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    return Ok(false);
                }
                _ => (),
            }
            return Ok(true);
        }
        if self.pane_focus != 0
            || self
                .repository_view()
                .is_some_and(|view| view.detail.is_some() || view.picking_branch)
        {
            return Ok(false);
        }
        if key.code == KeyCode::Char('/') {
            let target = self
                .settings
                .target(self.org.as_deref(), self.project.as_deref())?;
            let items = items.clone();
            self.scope_repository_filter(&target);
            if self
                .repo_cache
                .as_ref()
                .is_none_or(|(cached, _)| *cached != target)
            {
                self.repo_cache = Some((target, items));
            }
            self.repo_filter.editing = true;
            return Ok(true);
        }
        if key.code == KeyCode::Esc && !self.repo_filter.query.is_empty() {
            self.repo_filter.query.clear();
            self.apply_repository_filter()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn repository(&self) -> Result<(String, Value)> {
        let Screen::Repos(items) = &self.screen else {
            anyhow::bail!("Select a repository first");
        };
        let repo = items
            .get(self.selection.selected().unwrap_or(0))
            .context("No repository selected")?;
        let target = self
            .settings
            .target(self.org.as_deref(), self.project.as_deref())?;
        // A JSON tuple cannot collide when organization or project names contain slashes.
        let key =
            serde_json::to_string(&(&target.organization, &target.project, text(repo, "id")))?;
        Ok((key, repo.clone()))
    }

    pub(super) fn repository_view(&self) -> Option<&View> {
        self.repository()
            .ok()
            .and_then(|(key, _)| self.repo_views.get(&key))
    }

    fn repository_view_mut(&mut self) -> Result<&mut View> {
        let (key, repo) = self.repository()?;
        Ok(self
            .repo_views
            .entry(key)
            .or_insert_with(|| View::new(&repo)))
    }

    fn load_repository(&mut self, more: bool, refresh: bool) -> Result<()> {
        let (key, repo) = self.repository()?;
        let view = self.repository_view_mut()?;
        if view.picking_branch {
            if !refresh && view.branches.is_some() {
                return Ok(());
            }
        } else if view.tab == 1
            || view.tab == 2 && !refresh && view.tags.is_some()
            || view.tab == 0
                && (view.branch.is_empty()
                    || !refresh && !more && view.histories.contains_key(&view.branch))
        {
            return Ok(());
        }
        let branches = view.picking_branch;
        let tags = view.tab == 2 && !branches;
        let branch = view.branch.clone();
        let history = view.histories.get(&branch).filter(|_| more);
        if history.is_some_and(|h| !h.more) {
            return Ok(());
        }
        let skip = history.map_or(0, |h| h.commits.len());
        let pinned = history
            .and_then(|h| h.commits.first())
            .map(|c| text(c, "commitId").to_owned());
        let api = self.api()?;
        self.launch(async move {
            let reply = if tags || branches {
                Reply::Refs {
                    branches,
                    values: api.repo_refs(text(&repo, "id"), tags).await?,
                }
            } else {
                let values = api
                    .repo_commits(
                        text(&repo, "id"),
                        pinned.as_deref().unwrap_or(&branch),
                        pinned.is_some(),
                        skip,
                    )
                    .await?;
                Reply::Commits {
                    branch,
                    skip,
                    values,
                }
            };
            Ok(Message::Repository { key, reply })
        });
        self.loading_label = if branches {
            "Loading branches"
        } else if tags {
            "Loading tags"
        } else {
            "Loading commits"
        };
        Ok(())
    }

    pub(super) fn receive_repository(&mut self, key: String, reply: Reply) {
        let Some(view) = self.repo_views.get_mut(&key) else {
            return;
        };
        match reply {
            Reply::TagWritten { change, refreshed } => {
                self.receive_tag_write(key, change, refreshed)
            }
            Reply::Commits {
                branch,
                skip,
                values,
            } => {
                let history = view.histories.entry(branch).or_default();
                let selected = history
                    .commits
                    .get(history.selection.selected().unwrap_or(0))
                    .map(|v| text(v, "commitId").to_owned());
                if skip == 0 {
                    history.commits.clear();
                } else if skip != history.commits.len() {
                    return;
                }
                history.more = values.len() == 100;
                history.commits.extend(values);
                history.selection.select(Some(
                    selected
                        .and_then(|id| {
                            history
                                .commits
                                .iter()
                                .position(|v| text(v, "commitId") == id)
                        })
                        .unwrap_or(0),
                ));
            }
            Reply::Refs { branches, values } => {
                let selected_name = if branches && view.branches.is_none() {
                    Some(format!("refs/heads/{}", view.branch))
                } else {
                    let previous = if branches {
                        view.branches.as_deref()
                    } else {
                        view.tags.as_deref()
                    }
                    .unwrap_or_default();
                    let collapsed = if branches {
                        &view.collapsed_branches
                    } else {
                        &view.collapsed_tags
                    };
                    let state = if branches {
                        &view.branch_selection
                    } else {
                        &view.tags_selection
                    };
                    ref_rows(previous, branches, collapsed)
                        .get(state.selected().unwrap_or(0))
                        .map(|row| row.path.clone())
                };
                let collapsed = if branches {
                    &view.collapsed_branches
                } else {
                    &view.collapsed_tags
                };
                let rows = ref_rows(&values, branches, collapsed);
                let index = rows
                    .iter()
                    .position(|row| selected_name.as_deref() == Some(&row.path))
                    .or_else(|| rows.iter().position(|r| r.index.is_some()))
                    .unwrap_or(0);
                if branches {
                    view.branches = Some(values);
                    view.branch_selection.select(Some(index));
                } else {
                    view.tags = Some(values);
                    view.tags_selection.select(Some(index));
                }
            }
            Reply::Changes(value) => {
                view.detail = Some(value);
                self.pane_focus = 0;
                self.pane_x = [0; 3];
                self.pane_y = [0; 3];
            }
            Reply::Diff {
                commit,
                index,
                diff,
            } => {
                if let Some(detail) = &mut view.detail
                    && text(&detail["detailChanges"], "head") == commit
                    && let Some(file) = detail["detailChanges"]["files"].get_mut(index)
                {
                    file["diff"] = serde_json::json!(diff);
                    if review::selected(detail) == index {
                        self.pane_focus = 1;
                        self.pane_y[1] = 0;
                    }
                }
            }
        }
    }

    fn open_repository_commit(&mut self) -> Result<()> {
        let (key, repo) = self.repository()?;
        let commit = self
            .repository_view_mut()?
            .selected_commit()
            .filter(|s| !s.is_empty())
            .context("No commit selected")?
            .to_owned();
        let api = self.api()?;
        self.launch(async move {
            Ok(Message::Repository {
                key,
                reply: Reply::Changes(api.repo_commit_changes(text(&repo, "id"), &commit).await?),
            })
        });
        self.loading_label = "Loading commit changes";
        Ok(())
    }

    fn repository_diff(&mut self) -> Result<()> {
        let (key, _) = self.repository()?;
        let view = self.repository_view_mut()?;
        let detail = view.detail.as_ref().context("Open a commit first")?;
        let index = review::selected(detail);
        let file = detail["detailChanges"]["files"]
            .get(index)
            .context("No changed file selected")?;
        if file["diff"].is_string() {
            self.pane_focus = 1;
            return Ok(());
        }
        let changes = detail["detailChanges"].clone();
        let commit = text(&changes, "head").to_owned();
        let api = self.api()?;
        self.launch(async move {
            Ok(Message::Repository {
                key,
                reply: Reply::Diff {
                    commit,
                    index,
                    diff: api.pr_file_diff(&changes, index).await?,
                },
            })
        });
        self.loading_label = "Loading commit diff";
        Ok(())
    }

    pub(super) fn refresh_repository(&mut self) -> Result<()> {
        self.cancel_task();
        let view = self.repository_view_mut()?;
        if view.detail.is_some() {
            return self.open_repository_commit();
        }
        if !view.picking_branch && view.tab == 1 {
            return self.reload_repositories();
        }
        self.load_repository(false, true)
    }

    pub(super) fn handle_repository(&mut self, key: event::KeyEvent) -> Result<bool> {
        if self.repository().is_err() {
            return Ok(false);
        }
        let detail = self.repository_view_mut()?.detail.is_some();
        let picking = self.repository_view_mut()?.picking_branch;
        let tab = self.repository_view_mut()?.tab;
        if !detail
            && !picking
            && self.pane_focus == 1
            && self.task.is_none()
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Char('n') if tab == 0 || tab == 2 => {
                    self.open_tag_form(false)?;
                    return Ok(true);
                }
                KeyCode::Char('e') if tab == 2 => {
                    self.open_tag_form(true)?;
                    return Ok(true);
                }
                KeyCode::Char('d') | KeyCode::Delete if tab == 2 => {
                    self.confirm_tag_delete()?;
                    return Ok(true);
                }
                _ => (),
            }
        }
        let direct_tab = if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            None
        } else {
            match key.code {
                KeyCode::Char('c') => Some(0),
                KeyCode::Char('i') => Some(1),
                KeyCode::Char('t') => Some(2),
                _ => None,
            }
        };
        if direct_tab.is_some()
            || matches!(
                key.code,
                KeyCode::F(7) | KeyCode::F(8) | KeyCode::Char('[' | ']')
            )
        {
            self.cancel_task();
            let view = self.repository_view_mut()?;
            view.tab = direct_tab.unwrap_or_else(|| {
                (view.tab
                    + if matches!(key.code, KeyCode::F(8) | KeyCode::Char(']')) {
                        1
                    } else {
                        2
                    })
                    % 3
            });
            view.picking_branch = false;
            view.detail = None;
            self.pane_focus = 1;
            self.detail_scroll = 0;
            self.load_repository(false, false)?;
            return Ok(true);
        }
        if key.code == KeyCode::F(5) {
            if self.pane_focus == 1 || detail || picking {
                self.refresh_repository()?;
            } else {
                self.refresh()?;
            }
            return Ok(true);
        }
        if key.code == KeyCode::Esc && (picking || detail) {
            self.cancel_task();
            let view = self.repository_view_mut()?;
            view.picking_branch = false;
            view.detail = None;
            self.pane_focus = 1;
            return Ok(true);
        }
        if picking && matches!(key.code, KeyCode::Char('w') | KeyCode::F(6)) {
            self.cancel_task();
            self.repository_view_mut()?.picking_branch = false;
            self.pane_focus = 0;
            return Ok(true);
        }
        if key.code == KeyCode::Esc && self.pane_focus == 1 && self.task.is_none() {
            self.pane_focus = 0;
            return Ok(true);
        }
        if detail {
            if key.code == KeyCode::Enter && self.task.is_none() {
                self.repository_diff()?;
                return Ok(true);
            }
            let step = if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                20
            } else {
                1
            };
            let direction = match key.code {
                KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => Some(false),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => Some(true),
                _ => None,
            };
            if let Some(down) = direction {
                if self.pane_focus == 0 {
                    let detail = self
                        .repository_view_mut()?
                        .detail
                        .as_mut()
                        .context("Open a commit first")?;
                    let count = pr_rows(&detail["detailChanges"]["files"]).len();
                    let index = review::selected(detail);
                    detail["detailChangeIndex"] = serde_json::json!(if down {
                        index.saturating_add(step).min(count.saturating_sub(1))
                    } else {
                        index.saturating_sub(step)
                    });
                    self.pane_y[1] = 0;
                } else {
                    self.pane_y[1] = if down {
                        self.pane_y[1].saturating_add(step as u16)
                    } else {
                        self.pane_y[1].saturating_sub(step as u16)
                    };
                }
                return Ok(true);
            }
            if key.code == KeyCode::Home {
                if self.pane_focus == 0 {
                    if let Some(detail) = &mut self.repository_view_mut()?.detail {
                        detail["detailChangeIndex"] = serde_json::json!(0);
                    }
                } else {
                    self.pane_y[1] = 0;
                }
                return Ok(true);
            }
        }
        if !detail && key.code == KeyCode::Char('r') {
            self.cancel_task();
            self.repository_view_mut()?.picking_branch = true;
            self.pane_focus = 1;
            self.load_repository(false, false)?;
            return Ok(true);
        }
        if !detail && !picking && self.pane_focus == 0 {
            if matches!(
                key.code,
                KeyCode::Right | KeyCode::Enter | KeyCode::Char('w') | KeyCode::F(6)
            ) && !key.modifiers.contains(KeyModifiers::SHIFT)
            {
                self.pane_focus = 1;
                self.load_repository(false, false)?;
                return Ok(true);
            }
            if matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Char('j' | 'k')
                    | KeyCode::PageUp
                    | KeyCode::PageDown
            ) {
                self.cancel_task();
            }
            return Ok(false);
        }
        if key.code == KeyCode::Char('b')
            && !picking
            && let Some(commit) = self
                .repository_view_mut()?
                .selected_commit()
                .map(str::to_owned)
        {
            let (_, repo) = self.repository()?;
            let target = self
                .settings
                .target(self.org.as_deref(), self.project.as_deref())?;
            crate::external::browse(&format!(
                "{}/commit/{}",
                crate::external::repo_url(&target, &repo)?,
                crate::service::segment(&commit)
            ))?;
            return Ok(true);
        }
        if detail {
            return Ok(false);
        }
        let tab = self.repository_view_mut()?.tab;
        if tab == 1 && !picking {
            return Ok(false);
        }
        let step = if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            20
        } else {
            1
        };
        let direction = match key.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => Some(false),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => Some(true),
            _ => None,
        };
        if let Some(down) = direction {
            let view = self.repository_view_mut()?;
            let count = if picking || tab == 2 {
                view.rows().len()
            } else {
                view.histories
                    .get(&view.branch)
                    .map_or(0, |h| h.commits.len())
            };
            let state = if picking || tab == 2 {
                view.ref_selection()
            } else if let Some(history) = view.histories.get_mut(&view.branch) {
                &mut history.selection
            } else {
                return Ok(true);
            };
            let index = state.selected().unwrap_or(0);
            state.select(Some(if down {
                index.saturating_add(step).min(count.saturating_sub(1))
            } else {
                index.saturating_sub(step)
            }));
            return Ok(true);
        }
        if key.code == KeyCode::Home {
            let view = self.repository_view_mut()?;
            if picking || tab == 2 {
                view.ref_selection().select(Some(0));
            } else if let Some(h) = view.histories.get_mut(&view.branch) {
                h.selection.select(Some(0));
            }
            return Ok(true);
        }
        if (picking || tab == 2)
            && !key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Enter)
        {
            let view = self.repository_view_mut()?;
            let rows = view.rows();
            let selected = view.ref_selection().selected().unwrap_or(0);
            if let Some(row) = rows.get(selected) {
                if row.index.is_none() {
                    let collapsed = if picking {
                        &mut view.collapsed_branches
                    } else {
                        &mut view.collapsed_tags
                    };
                    if key.code == KeyCode::Left {
                        collapsed.insert(row.path.clone());
                    } else if key.code == KeyCode::Right {
                        collapsed.remove(&row.path);
                    } else if !collapsed.remove(&row.path) {
                        collapsed.insert(row.path.clone());
                    }
                    return Ok(true);
                }
                if key.code == KeyCode::Left {
                    if let Some(parent) = rows[..selected]
                        .iter()
                        .rposition(|r| r.index.is_none() && r.depth < row.depth)
                    {
                        view.ref_selection().select(Some(parent));
                        return Ok(true);
                    }
                } else if key.code == KeyCode::Enter {
                    if picking {
                        let index = row.index.context("No branch selected")?;
                        view.branch = text(&view.refs()[index], "name")
                            .trim_start_matches("refs/heads/")
                            .into();
                        view.picking_branch = false;
                        view.tab = 0;
                        self.cancel_task();
                        self.load_repository(false, false)?;
                    } else if self.task.is_none() {
                        self.open_repository_commit()?;
                    }
                    return Ok(true);
                }
            }
        }
        if tab == 0 && !picking {
            match key.code {
                KeyCode::Enter if self.task.is_none() => {
                    self.open_repository_commit()?;
                    return Ok(true);
                }
                KeyCode::Char('m') if self.task.is_none() => {
                    self.load_repository(true, false)?;
                    return Ok(true);
                }
                _ => (),
            }
        }
        Ok(false)
    }
}

fn short(value: &str) -> String {
    value.chars().take(7).collect()
}
fn date(value: &Value) -> String {
    commit_date(value)
        .map(|d| d.date().to_string())
        .unwrap_or_else(|| "Unknown date".into())
}

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App, repos: &[Value]) {
    let filtering = app.repo_filter.editing || !app.repo_filter.query.is_empty();
    let layout = Layout::vertical([Constraint::Length(u16::from(filtering)), Constraint::Min(0)])
        .split(area);
    if filtering {
        let total = app
            .repo_cache
            .as_ref()
            .map_or(repos.len(), |(_, items)| items.len());
        let count = format!("{} / {total}", repos.len());
        let fields = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(count.len() as u16 + 2),
        ])
        .split(layout[0]);
        let input = format!("Filter: {}", app.repo_filter.query);
        let width = Line::raw(&input).width() as u16;
        let scroll = width.saturating_sub(fields[0].width.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(input)
                .scroll((0, scroll))
                .style(Style::default().fg(if app.repo_filter.editing {
                    Color::Cyan
                } else {
                    Color::Gray
                })),
            fields[0],
        );
        frame.render_widget(Paragraph::new(count).right_aligned(), fields[1]);
        if app.repo_filter.editing && fields[0].width > 0 && fields[0].height > 0 {
            frame.set_cursor_position((fields[0].x + width - scroll, fields[0].y));
        }
    }
    let area = layout[1];
    let Some(repo) = repos.get(app.selection.selected().unwrap_or(0)) else {
        frame.render_widget(
            Paragraph::new(if app.task.is_some() {
                "Loading repositories..."
            } else if !app.repo_filter.query.is_empty() {
                "No repositories match the filter. Press Esc to clear."
            } else {
                "No repositories in this project. Press F5 to refresh."
            })
            .block(pane_block("Repositories", true)),
            area,
        );
        return;
    };
    if let Some(detail) = app.repository_view().and_then(|v| v.detail.as_ref()) {
        review::render(frame, area, detail, app.pane_focus, app.pane_y, app.pane_x);
        return;
    }
    let panes = if area.width >= 90 {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area)
    } else {
        Layout::vertical([
            Constraint::Length(if area.height >= 22 { 7 } else { 5 }),
            Constraint::Min(0),
        ])
        .split(area)
    };
    draw_list(
        frame,
        panes[0],
        pane_block("Repositories", app.pane_focus == 0),
        repos
            .iter()
            .map(|v| {
                format!(
                    "{}{}",
                    text(v, "name"),
                    if v["isDisabled"] == true {
                        " (disabled)"
                    } else {
                        ""
                    }
                )
            })
            .collect(),
        &mut app.selection,
        app.pane_x[0],
    );
    let focus = app.pane_focus == 1;
    let detail_scroll = app.detail_scroll;
    let horizontal = app.pane_x[1];
    let loading = app.task.is_some();
    let block = pane_block(output::cell(text(repo, "name")), focus);
    let inner = block.inner(panes[1]);
    frame.render_widget(block, panes[1]);
    let layout = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(inner);
    let Ok(view) = app.repository_view_mut() else {
        return;
    };
    frame.render_widget(
        Paragraph::new(Line::from(
            ["Commits [c]", "Info [i]", "Tags [t]"]
                .into_iter()
                .enumerate()
                .flat_map(|(i, s)| {
                    [
                        Span::styled(
                            format!(" {} ", s),
                            if view.tab == i {
                                Style::default().fg(Color::Cyan).underlined()
                            } else {
                                Style::default().fg(Color::DarkGray)
                            },
                        ),
                        Span::raw(" "),
                    ]
                })
                .collect::<Vec<_>>(),
        )),
        layout[0],
    );
    if view.picking_branch {
        render_refs(frame, layout[1], view, horizontal, loading);
    } else if view.tab == 1 {
        let branch = text(repo, "defaultBranch").trim_start_matches("refs/heads/");
        frame.render_widget(Paragraph::new(output::clean(&format!("Default branch: {}\nStatus: {}\nID: {}\n\nHTTPS clone\n{}\n\nSSH clone\n{}\n\nWeb\n{}", if branch.is_empty() { "Not set" } else { branch }, if repo["isDisabled"] == true { "Disabled" } else { "Enabled" }, text(repo,"id"), text(repo,"remoteUrl"), text(repo,"sshUrl"), text(repo,"webUrl")))).wrap(Wrap { trim: false }).scroll((detail_scroll,horizontal)), layout[1]);
    } else if view.tab == 0 {
        render_commits(frame, layout[1], view, horizontal, loading);
    } else {
        render_refs(frame, layout[1], view, horizontal, loading);
    }
}

fn pan(value: &str, horizontal: u16) -> String {
    output::cell(value)
        .chars()
        .skip(usize::from(horizontal))
        .collect()
}

fn render_commits(frame: &mut Frame, area: Rect, view: &mut View, horizontal: u16, loading: bool) {
    let layout = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(if area.height >= 13 { 5 } else { 0 }),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(output::cell(&format!(
            "Branch: {} [r] · newest first",
            if view.branch.is_empty() {
                "Not set"
            } else {
                &view.branch
            }
        ))),
        layout[0],
    );
    let Some(history) = view.histories.get_mut(&view.branch) else {
        frame.render_widget(
            Paragraph::new(if loading {
                "Loading commits..."
            } else if view.branch.is_empty() {
                "No default branch. Press r to choose a branch."
            } else {
                "Commits not loaded. Press F5 to load."
            })
            .wrap(Wrap { trim: false }),
            layout[1],
        );
        return;
    };
    let mut previous = String::new();
    let items: Vec<_> = history
        .commits
        .iter()
        .map(|c| {
            let day = date(c);
            let mut lines = Vec::new();
            if day != previous {
                lines.push(Line::styled(
                    format!("── {day} ──"),
                    Style::default().fg(Color::DarkGray),
                ));
                previous = day;
            }
            lines.push(Line::raw(pan(
                text(c, "comment").lines().next().unwrap_or("(no message)"),
                horizontal,
            )));
            lines.push(Line::styled(
                pan(
                    &format!(
                        "{} · {}",
                        text(&c["author"], "name"),
                        short(text(c, "commitId"))
                    ),
                    horizontal,
                ),
                Style::default().fg(Color::Cyan),
            ));
            ListItem::new(lines)
        })
        .collect();
    if items.is_empty() {
        frame.render_widget(Paragraph::new("No commits on this branch."), layout[1]);
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .highlight_style(selection_style())
                .highlight_symbol("› ")
                .highlight_spacing(ratatui::widgets::HighlightSpacing::Always),
            layout[1],
            &mut history.selection,
        );
    }
    let c = history
        .commits
        .get(history.selection.selected().unwrap_or(0));
    let preview = c
        .map(|c| {
            format!(
                "Commit {}\n{}\n{} · {}\n{} commits loaded{}",
                short(text(c, "commitId")),
                text(c, "comment"),
                text(&c["author"], "name"),
                date(c),
                history.commits.len(),
                if history.more { " · More [m]" } else { "" }
            )
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(output::clean(&preview))
            .block(Block::default().borders(Borders::TOP))
            .wrap(Wrap { trim: false }),
        layout[2],
    );
}

fn render_refs(frame: &mut Frame, area: Rect, view: &mut View, horizontal: u16, loading: bool) {
    let branches = view.picking_branch;
    let layout = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(if area.height >= 13 { 5 } else { 0 }),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(if branches {
            "Branches by path · Enter: choose · Esc: close"
        } else {
            "Tags by path · newest first per folder"
        })
        .wrap(Wrap { trim: false }),
        layout[0],
    );
    let loaded = if branches {
        view.branches.is_some()
    } else {
        view.tags.is_some()
    };
    let rows = view.rows();
    let collapsed = if branches {
        &view.collapsed_branches
    } else {
        &view.collapsed_tags
    };
    let items: Vec<_> = rows
        .iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth);
            let line = if let Some(index) = row.index {
                let value = &view.refs()[index];
                format!(
                    "{indent}  {}{}",
                    row.name,
                    if branches {
                        if text(value, "name").trim_start_matches("refs/heads/") == view.branch {
                            "  [current]".into()
                        } else {
                            String::new()
                        }
                    } else {
                        format!(
                            "  {}  {}",
                            date(&value["targetCommit"]),
                            short(ref_commit(value))
                        )
                    }
                )
            } else {
                format!(
                    "{indent}{} {}  ({})",
                    if collapsed.contains(&row.path) {
                        "▸"
                    } else {
                        "▾"
                    },
                    row.name,
                    row.count
                )
            };
            ListItem::new(pan(&line, horizontal))
        })
        .collect();
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(if loading {
                "Loading..."
            } else if !loaded {
                "Not loaded. Press F5 to load."
            } else if branches {
                "No branches in this repository."
            } else {
                "No tags in this repository."
            }),
            layout[1],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .highlight_style(selection_style())
                .highlight_symbol("› "),
            layout[1],
            view.ref_selection(),
        );
    }
    let state = if branches {
        &view.branch_selection
    } else {
        &view.tags_selection
    };
    let preview = rows
        .get(state.selected().unwrap_or(0))
        .map(|r| {
            if let Some(index) = r.index {
                let v = &view.refs()[index];
                format!(
                    "{}\nCommit {}\n{}",
                    text(v, "name").trim_start_matches(if branches {
                        "refs/heads/"
                    } else {
                        "refs/tags/"
                    }),
                    short(ref_commit(v)),
                    text(&v["targetCommit"], "comment")
                )
            } else {
                format!(
                    "{} · {} {}\nEnter / ← / →: collapse or expand",
                    r.name,
                    r.count,
                    if branches { "branches" } else { "tags" }
                )
            }
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(output::clean(&preview))
            .block(Block::default().borders(Borders::TOP))
            .wrap(Wrap { trim: false }),
        layout[2],
    );
}
