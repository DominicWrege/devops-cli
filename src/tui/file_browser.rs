use super::{chrome::*, output};
use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Clear, List, ListItem, ListState, Paragraph},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Directory,
    File,
}

struct Entry {
    path: PathBuf,
    directory: bool,
}

pub(super) enum Action {
    Stay,
    Cancel,
    Selected(PathBuf),
}

pub(super) struct Browser {
    kind: Kind,
    directory: PathBuf,
    entries: Vec<Entry>,
    selected: usize,
    error: String,
}

impl Browser {
    pub(super) fn new(value: &str, kind: Kind) -> Result<Self> {
        let mut path = std::path::absolute(if value.is_empty() {
            Path::new(".")
        } else {
            Path::new(value)
        })?;
        let selected = path.is_file().then(|| path.clone());
        while !path.is_dir() {
            if !path.pop() {
                path = std::env::current_dir()?;
                break;
            }
        }
        let mut browser = Self {
            kind,
            directory: path.clone(),
            entries: Vec::new(),
            selected: 0,
            error: String::new(),
        };
        browser.load(&path)?;
        if let Some(selected) = selected {
            browser.selected = browser
                .entries
                .iter()
                .position(|entry| entry.path == selected)
                .unwrap_or(0);
        }
        Ok(browser)
    }

    fn load(&mut self, directory: &Path) -> Result<()> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(directory)
            .with_context(|| format!("Cannot read {}", directory.display()))?
        {
            let entry = entry.context("Cannot read directory entry")?;
            let path = entry.path();
            let directory = path.is_dir();
            if directory || (self.kind == Kind::File && path.is_file()) {
                entries.push(Entry { path, directory });
            }
        }
        entries.sort_by_cached_key(|entry| {
            (
                !entry.directory,
                entry
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase(),
            )
        });
        if let Some(parent) = directory.parent() {
            entries.insert(
                0,
                Entry {
                    path: parent.to_owned(),
                    directory: true,
                },
            );
        }
        self.directory = directory.to_owned();
        self.entries = entries;
        self.selected = 0;
        self.error.clear();
        Ok(())
    }

    fn parent(&mut self) -> Result<()> {
        if let Some(parent) = self.directory.parent().map(Path::to_owned) {
            let previous = self.directory.clone();
            self.load(&parent)?;
            self.selected = self
                .entries
                .iter()
                .position(|entry| entry.path == previous)
                .unwrap_or(0);
        }
        Ok(())
    }

    fn choose(&self) -> Result<Action> {
        let path = if self.kind == Kind::Directory {
            ensure!(self.directory.is_dir(), "Directory no longer exists");
            self.directory.clone()
        } else {
            let entry = self
                .entries
                .get(self.selected)
                .context("Select an editor file")?;
            ensure!(
                !entry.directory && entry.path.is_file(),
                "Select an editor file"
            );
            entry.path.clone()
        };
        ensure!(
            path.to_str().is_some(),
            "Selected path is not UTF-8; enter it manually"
        );
        Ok(Action::Selected(path))
    }

    fn handle(&mut self, key: KeyEvent) -> Result<Action> {
        match key.code {
            KeyCode::Esc => return Ok(Action::Cancel),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1))
            }
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => {
                self.selected = (self.selected + 10).min(self.entries.len().saturating_sub(1))
            }
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.entries.len().saturating_sub(1),
            KeyCode::Left | KeyCode::Backspace => self.parent()?,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return self.choose();
            }
            KeyCode::Enter | KeyCode::Right => {
                if let Some(entry) = self.entries.get(self.selected) {
                    if entry.directory {
                        self.load(&entry.path.clone())?;
                    } else if key.code == KeyCode::Enter {
                        return self.choose();
                    }
                }
            }
            _ => (),
        }
        Ok(Action::Stay)
    }

    pub(super) fn key(&mut self, key: KeyEvent) -> Action {
        match self.handle(key) {
            Ok(action) => action,
            Err(error) => {
                self.error = format!("{error:#}");
                Action::Stay
            }
        }
    }

    pub(super) fn draw(&self, frame: &mut Frame, area: Rect) {
        let rect = popup(area, 88, area.height.min(24));
        let block = pane_block(
            if self.kind == Kind::Directory {
                "Choose directory"
            } else {
                "Choose editor executable"
            },
            true,
        );
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(u16::from(!self.error.is_empty())),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(output::cell(&self.directory.display().to_string())),
            layout[0],
        );
        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|entry| {
                let label = if Some(entry.path.as_path()) == self.directory.parent() {
                    "../".to_owned()
                } else {
                    format!(
                        "{}{}",
                        output::cell(&entry.path.file_name().unwrap_or_default().to_string_lossy()),
                        if entry.directory { "/" } else { "" }
                    )
                };
                ListItem::new(label).style(Style::default().fg(if entry.directory {
                    Color::Cyan
                } else {
                    Color::Gray
                }))
            })
            .collect();
        if items.is_empty() {
            frame.render_widget(Paragraph::new("No entries"), layout[1]);
        } else {
            let mut state = ListState::default().with_selected(Some(self.selected));
            frame.render_stateful_widget(
                List::new(items).highlight_style(selection_style()),
                layout[1],
                &mut state,
            );
        }
        frame.render_widget(
            Paragraph::new(output::cell(&self.error)).style(Style::default().fg(Color::Red)),
            layout[2],
        );
    }
}
