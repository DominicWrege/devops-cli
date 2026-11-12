use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Pane(usize),
    Row(usize),
    Task(usize),
    Run(usize),
    File(usize),
    Key(KeyCode),
}

#[derive(Default)]
pub(super) struct State {
    pub regions: Vec<(Rect, usize, Target)>,
    pub task_offset: usize,
    last_click: Option<(Target, Instant)>,
}

impl State {
    fn pane(&mut self, area: Rect, focus: usize) {
        self.regions.push((area, focus, Target::Pane(focus)));
    }

    fn pr_tabs(&mut self, area: Rect, focus: usize, bordered: bool, actions_enabled: bool) {
        let mut x = area.x.saturating_add(u16::from(bordered));
        let right = area.right().saturating_sub(u16::from(bordered));
        for (index, span) in super::pr_tabs(0, area.width, actions_enabled)
            .spans
            .iter()
            .enumerate()
        {
            let width = (span.width() as u16).min(right.saturating_sub(x));
            if index % 2 == 0 && (index < 8 || actions_enabled) {
                let key = ['d', 'c', 't', 'f', 'm', 'v'][index / 2];
                self.regions.push((
                    Rect::new(x, area.y, width, area.height.min(1)),
                    focus,
                    Target::Key(KeyCode::Char(key)),
                ));
            }
            x = x.saturating_add(span.width() as u16);
        }
    }

    fn rows(
        &mut self,
        area: Rect,
        focus: usize,
        offset: usize,
        count: usize,
        table: bool,
        target: fn(usize) -> Target,
    ) {
        self.pane(area, focus);
        let inner = Block::bordered().inner(area);
        let header = u16::from(table).min(inner.height);
        for row in 0..inner.height.saturating_sub(header) {
            let index = offset + usize::from(row);
            if index >= count {
                break;
            }
            self.regions.push((
                Rect::new(inner.x, inner.y + header + row, inner.width, 1),
                focus,
                target(index),
            ));
        }
    }
}

pub(super) fn register(app: &mut App, header: Rect, area: Rect) {
    // Modal input must never reach the screen behind it.
    if app.form.is_some()
        || app.confirm.is_some()
        || app.tag_confirm.is_some()
        || app.settings_menu.is_some()
        || app.pr_actions
        || app.help_scroll.is_some()
    {
        return;
    }
    if !matches!(app.screen, Screen::Organizations(_) | Screen::Projects(_)) {
        let labels = chrome::section_labels(header.width);
        let mut x = header.x;
        for (i, label) in labels.iter().enumerate() {
            let width = (label.len() as u16).min(header.right().saturating_sub(x));
            app.mouse.regions.push((
                Rect::new(x, header.y, width, header.height.min(1)),
                0,
                Target::Key(KeyCode::Char(char::from(b'1' + i as u8))),
            ));
            x = x.saturating_add(label.len() as u16 + 3);
        }
    }
    let offset = app.selection.offset();
    match &app.screen {
        Screen::Organizations(items) | Screen::Projects(items) => {
            let rect = popup(
                area,
                72,
                (items.len() as u16).saturating_add(4).clamp(6, 20),
            );
            app.mouse
                .rows(rect, 0, offset, items.len(), false, Target::Row);
        }
        Screen::Recent(items) | Screen::Runs { values: items, .. } => {
            app.mouse
                .rows(area, 0, offset, items.len(), true, Target::Row)
        }
        Screen::Pipelines(items) => {
            let panes = if area.width >= 90 {
                Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .split(area)
            } else {
                Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)])
                    .split(area)
            };
            app.mouse
                .rows(panes[0], 0, offset, items.len(), true, Target::Row);
            app.mouse.pane(panes[1], 1);
        }
        Screen::Prs(items) => {
            let mut list = area;
            if area.width >= 90 {
                let panes =
                    Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                        .split(area);
                list = panes[0];
                app.mouse.pane(panes[1], 1);
                if area.height >= 12 {
                    list =
                        Layout::vertical([Constraint::Percentage(65), Constraint::Percentage(35)])
                            .split(list)[0];
                }
            }
            app.mouse
                .rows(list, 0, offset, items.len(), false, Target::Row);
        }
        Screen::Repos(items) => {
            // Repository detail views have their own file and diff layout.
            if app.repository_view().is_some_and(|v| v.detail.is_some()) {
                return;
            }
            let area = Layout::vertical([
                Constraint::Length(u16::from(
                    app.repo_filter.editing || !app.repo_filter.query.is_empty(),
                )),
                Constraint::Min(0),
            ])
            .split(area)[1];
            let panes = if area.width >= 90 {
                Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .split(area)
            } else {
                Layout::vertical([
                    Constraint::Length(if area.height >= 22 { 7 } else { 5 }),
                    Constraint::Min(0),
                ])
                .split(area)
            };
            app.mouse
                .rows(panes[0], 0, offset, items.len(), false, Target::Row);
            app.mouse.pane(panes[1], 1);
        }
        Screen::Pr { value } => {
            if app.pr_tab == 3 {
                let layout =
                    Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
                let panes = if area.width >= 90 {
                    Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                        .split(layout[1])
                } else {
                    Layout::vertical([Constraint::Percentage(35), Constraint::Percentage(65)])
                        .split(layout[1])
                };
                let height = Block::bordered().inner(panes[0]).height as usize;
                let offset = review::selected(value).saturating_sub(height.saturating_sub(1));
                app.mouse.rows(
                    panes[0],
                    0,
                    offset,
                    pr_rows(&value["detailChanges"]["files"]).len(),
                    false,
                    Target::File,
                );
                app.mouse.pane(panes[1], 1);
                app.mouse
                    .pr_tabs(layout[0], 0, false, value["status"] == "active");
            } else {
                let wide = area.width >= 90;
                let columns =
                    Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                        .split(area);
                let regions = Layout::vertical([Constraint::Length(5), Constraint::Min(0)])
                    .split(if wide { columns[0] } else { area });
                if wide {
                    app.mouse.pane(regions[1], 0);
                }
                let detail = if wide { columns[1] } else { regions[1] };
                app.mouse.pane(detail, usize::from(wide));
                app.mouse
                    .pr_tabs(detail, usize::from(wide), true, value["status"] == "active");
            }
        }
        Screen::Watch(viewer) => {
            let [tasks, logs, runs] = viewer_areas(area);
            app.mouse.rows(
                tasks,
                0,
                app.mouse.task_offset,
                viewer.records.len(),
                false,
                Target::Task,
            );
            app.mouse.pane(logs, 1);
            if let Some(list) = &app.run_list {
                app.mouse.rows(
                    runs,
                    2,
                    list.selection.offset(),
                    list.items.len(),
                    false,
                    Target::Run,
                );
            } else {
                app.mouse.pane(runs, 2);
            }
        }
        _ => (),
    }
}

impl App {
    pub(super) fn handle_mouse(&mut self, event: MouseEvent) -> Result<bool> {
        let direction = match event.kind {
            MouseEventKind::ScrollUp => Some(KeyCode::Up),
            MouseEventKind::ScrollDown => Some(KeyCode::Down),
            _ => None,
        };
        if self.help_scroll.is_some() {
            if let Some(code) = direction {
                return self.handle(Event::Key(event::KeyEvent::new(code, KeyModifiers::NONE)));
            }
            return Ok(false);
        }
        if self.form.is_some()
            || self.confirm.is_some()
            || self.tag_confirm.is_some()
            || self.settings_menu.is_some()
            || self.pr_actions
            || self.writing
        {
            return Ok(false);
        }
        if direction.is_none() && event.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(false);
        }
        let point = Position::new(event.column, event.row);
        let Some((_, focus, target)) = self
            .mouse
            .regions
            .iter()
            .rev()
            .find(|(area, _, _)| area.contains(point))
            .copied()
        else {
            return Ok(false);
        };
        if let Some(code) = direction {
            self.mouse.last_click = None;
            if matches!(target, Target::Key(_)) {
                return Ok(false);
            }
            self.pane_focus = focus;
            return self.handle(Event::Key(event::KeyEvent::new(code, KeyModifiers::NONE)));
        }
        if let Target::Key(code) = target {
            self.mouse.last_click = None;
            if matches!(code, KeyCode::Char('1' | '2' | '3' | '4')) {
                self.repo_filter.editing = false;
            }
            return self.handle(Event::Key(event::KeyEvent::new(code, KeyModifiers::NONE)));
        }
        if self.task.is_some()
            && !matches!(
                self.screen,
                Screen::Watch(_) | Screen::Pipelines(_) | Screen::Recent(_)
            )
            && !(matches!(self.screen, Screen::Pr { .. })
                && self.pr_tab == 3
                && matches!(target, Target::File(_) | Target::Pane(_)))
        {
            return Ok(false);
        }
        self.pane_focus = focus;
        let double = self.mouse.last_click.is_some_and(|(previous, at)| {
            previous == target && at.elapsed() <= Duration::from_millis(400)
        });
        self.mouse.last_click = if double {
            None
        } else {
            Some((target, Instant::now()))
        };
        match target {
            Target::Row(index) => {
                self.selection.select(Some(index));
                self.detail_scroll = 0;
                if matches!(self.screen, Screen::Pipelines(_)) {
                    self.load_activity()?;
                }
            }
            Target::Task(index) => {
                if let Screen::Watch(viewer) = &mut self.screen {
                    viewer.navigate(index as isize - viewer.selection() as isize);
                    self.poll_at = Instant::now();
                }
            }
            Target::File(index) => {
                if let Screen::Pr { value } = &mut self.screen {
                    value["detailChangeIndex"] = serde_json::json!(index);
                    self.pane_y[1] = 0;
                    self.pane_x[1] = 0;
                    self.load_diff()?;
                }
            }
            Target::Run(index) => {
                if let Some(list) = &mut self.run_list {
                    list.selection.select(Some(index));
                }
            }
            _ => (),
        }
        if double && matches!(target, Target::Row(_) | Target::Run(_) | Target::File(_)) {
            return self.handle(Event::Key(event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )));
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(app: &mut App, width: u16, height: u16) -> Result<Buffer> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
        terminal.draw(|frame| draw(frame, app))?;
        Ok(terminal.backend().buffer().clone())
    }

    fn at(buffer: &Buffer, text: &str) -> Result<Position> {
        for y in 0..buffer.area.height {
            let row: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            if let Some(byte) = row.find(text) {
                return Ok(Position::new(Line::raw(&row[..byte]).width() as u16, y));
            }
        }
        anyhow::bail!("Missing rendered text: {text}");
    }

    fn mouse(app: &mut App, point: Position, kind: MouseEventKind) -> Result<bool> {
        app.handle(Event::Mouse(MouseEvent {
            kind,
            column: point.x,
            row: point.y,
            modifiers: KeyModifiers::NONE,
        }))
    }

    #[test]
    fn pr_tabs_actions_and_settings_are_clickable_at_narrow_and_wide_sizes() -> Result<()> {
        for width in [50, 70, 90, 120] {
            let (_root, mut app) = super::super::tests::fixture()?;
            app.screen = Screen::Pr {
                value: json!({"pullRequestId":7,"status":"active",
                    "detailChanges":{"files":[{"item":{"path":"/one"},"diff":"+one"}]}
                }),
            };
            for (label, tab) in [("Chat", 1), ("Stat", 2), ("Files", 3), ("Desc", 0)] {
                let buffer = render(&mut app, width, 30)?;
                mouse(
                    &mut app,
                    at(&buffer, label)?,
                    MouseEventKind::Down(MouseButton::Left),
                )?;
                assert_eq!(app.pr_tab, tab);
            }
            for tab in [0, 3] {
                app.pr_tab = tab;
                assert!(
                    !command_hints(&app)
                        .iter()
                        .any(|(hint, _)| hint == "Approve [v]" || hint == "Merge [m]")
                );
                let buffer = render(&mut app, width, 30)?;
                mouse(
                    &mut app,
                    at(&buffer, "Merge [m]")?,
                    MouseEventKind::Down(MouseButton::Left),
                )?;
                assert!(matches!(
                    app.form.as_ref().map(|f| &f.kind),
                    Some(FormKind::Merge(7))
                ));
                assert!(!app.pr_actions);
                app.handle(Event::Key(event::KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                )))?;
                assert!(matches!(app.confirm, Some(PrCommand::Merge { id: 7, .. })));
                assert!(app.task.is_none());
                app.handle(Event::Key(event::KeyEvent::new(
                    KeyCode::Esc,
                    KeyModifiers::NONE,
                )))?;
                let buffer = render(&mut app, width, 30)?;
                mouse(
                    &mut app,
                    at(&buffer, "Approve [v]")?,
                    MouseEventKind::Down(MouseButton::Left),
                )?;
                assert!(matches!(
                    app.confirm,
                    Some(PrCommand::Review {
                        id: 7,
                        vote: Vote::Approve
                    })
                ));
                assert!(app.task.is_none());
                app.handle(Event::Key(event::KeyEvent::new(
                    KeyCode::Esc,
                    KeyModifiers::NONE,
                )))?;
            }
            let buffer = render(&mut app, width, 30)?;
            mouse(
                &mut app,
                at(&buffer, "Settings [4]")?,
                MouseEventKind::Down(MouseButton::Left),
            )?;
            assert_eq!(app.settings_menu, Some(0));
            assert!(matches!(app.screen, Screen::Pr { .. }));
            let buffer = render(&mut app, width, 30)?;
            assert!(
                buffer[(at(&buffer, "Settings [4]")?.x, 0)]
                    .modifier
                    .contains(Modifier::UNDERLINED)
            );
            app.handle(Event::Key(event::KeyEvent::new(
                KeyCode::Char('4'),
                KeyModifiers::NONE,
            )))?;
            assert!(app.settings_menu.is_none());
            app.handle(Event::Key(event::KeyEvent::new(
                KeyCode::Char('4'),
                KeyModifiers::NONE,
            )))?;
            assert_eq!(app.settings_menu, Some(0));
            app.settings_menu = None;
            app.screen = Screen::Repos(vec![]);
            app.repo_filter.editing = true;
            app.repo_filter.query = "demo".into();
            let buffer = render(&mut app, width, 30)?;
            mouse(
                &mut app,
                at(&buffer, "Settings [4]")?,
                MouseEventKind::Down(MouseButton::Left),
            )?;
            assert_eq!(app.settings_menu, Some(0));
            assert!(!app.repo_filter.editing);
            assert_eq!(app.repo_filter.query, "demo");
        }
        Ok(())
    }

    #[tokio::test]
    async fn file_clicks_cancel_pending_diffs_and_keep_list_focus() -> Result<()> {
        for width in [70, 120] {
            let (_root, mut app) = super::super::tests::fixture()?;
            app.screen = Screen::Pr {
                value: json!({"pullRequestId":7,"status":"active",
                    "detailChanges":{"files":[
                        {"item":{"path":"/one.rs"},"diff":"+one"},
                        {"item":{"path":"/two.rs"},"diff":"+two"}
                    ]}
                }),
            };
            app.pr_tab = 3;
            app.launch(std::future::pending());
            let buffer = render(&mut app, width, 30)?;
            app.pane_x[1] = 8;
            app.pane_y[1] = 4;
            mouse(
                &mut app,
                at(&buffer, "/two.rs")?,
                MouseEventKind::Down(MouseButton::Left),
            )?;
            assert_eq!(app.pane_focus, 0);
            assert_eq!(app.pane_y[1], 0);
            assert_eq!(app.pane_x[1], 0);
            assert!(app.task.is_none());
            let Screen::Pr { value } = &app.screen else {
                anyhow::bail!("Expected PR")
            };
            assert_eq!(review::selected(value), 1);
            assert!(at(&render(&mut app, width, 30)?, "+two").is_ok());
        }
        Ok(())
    }

    #[test]
    fn completed_prs_disable_merge_and_approve_buttons() -> Result<()> {
        let (_root, mut app) = super::super::tests::fixture()?;
        app.screen = Screen::Pr {
            value: json!({"pullRequestId":7,"status":"completed"}),
        };
        let buffer = render(&mut app, 120, 30)?;
        for (label, key) in [("Merge [m]", 'm'), ("Approve [v]", 'v')] {
            let point = at(&buffer, label)?;
            assert_eq!(buffer[(point.x, point.y)].fg, Color::DarkGray);
            mouse(&mut app, point, MouseEventKind::Down(MouseButton::Left))?;
            assert!(app.form.is_none());
            assert!(app.open_pr_form(key).is_err());
            assert!(app.confirm.is_none());
        }
        Ok(())
    }

    #[test]
    fn clicks_select_visible_scrolled_rows_and_ignore_headers_and_blank_space() -> Result<()> {
        for width in [50, 120] {
            let (_root, mut app) = super::super::tests::fixture()?;
            app.screen = Screen::Recent(
                (0..40)
                    .map(|i| json!({"id":i,"buildNumber":format!("run-{i:02}")}))
                    .collect(),
            );
            app.selection.select(Some(30));
            let buffer = render(&mut app, width, 20)?;
            let position = at(&buffer, "run-29")?;
            mouse(&mut app, position, MouseEventKind::Down(MouseButton::Left))?;
            assert_eq!(app.selection.selected(), Some(29));
            mouse(
                &mut app,
                at(&buffer, "Status")?,
                MouseEventKind::Down(MouseButton::Left),
            )?;
            assert_eq!(app.selection.selected(), Some(29));
            mouse(&mut app, position, MouseEventKind::ScrollUp)?;
            assert_eq!(app.selection.selected(), Some(28));
            app.screen = Screen::Recent(vec![]);
            let buffer = render(&mut app, width, 20)?;
            mouse(
                &mut app,
                at(&buffer, "No items")?,
                MouseEventKind::Down(MouseButton::Left),
            )?;
            assert_eq!(app.selection.selected(), Some(28));
        }
        Ok(())
    }

    #[test]
    fn section_clicks_use_cached_navigation_and_dialogs_block_click_through() -> Result<()> {
        let (_root, mut app) = super::super::tests::fixture()?;
        let target = app.settings.target(None, None)?;
        app.pr_cache = Some((target.clone(), false, vec![json!({"title":"PR"})]));
        app.repo_cache = Some((target, vec![json!({"id":"repo","name":"Repo"})]));
        let buffer = render(&mut app, 120, 30)?;
        let prs = at(&buffer, "Pull requests [2]")?;
        mouse(&mut app, prs, MouseEventKind::Down(MouseButton::Left))?;
        assert!(matches!(app.screen, Screen::Prs(_)));
        render(&mut app, 120, 30)?;
        app.form = Some(Form::new(
            FormKind::Comment(7),
            vec![("Comment", String::new())],
        ));
        mouse(
            &mut app,
            Position::new(36, 0),
            MouseEventKind::Down(MouseButton::Left),
        )?;
        mouse(&mut app, Position::new(2, 3), MouseEventKind::ScrollDown)?;
        assert!(matches!(app.screen, Screen::Prs(_)));
        assert_eq!(app.form.as_ref().context("Expected form")?.selected, 0);
        app.form = None;
        app.writing = true;
        mouse(
            &mut app,
            Position::new(36, 0),
            MouseEventKind::Down(MouseButton::Left),
        )?;
        assert!(matches!(app.screen, Screen::Prs(_)));
        Ok(())
    }

    #[test]
    fn task_clicks_and_log_wheel_preserve_following_rules_at_both_widths() -> Result<()> {
        for width in [70, 120] {
            let (_root, mut app) = super::super::tests::fixture()?;
            let mut viewer = Viewer::new(7);
            viewer.update(json!({"id":7}), (0..40).map(|i| json!({"id":format!("task-{i}"),"type":"Task","name":format!("Task {i:02}"),"state":"inProgress"})).collect());
            viewer.navigate(39);
            app.screen = Screen::Watch(Box::new(viewer));
            let buffer = render(&mut app, width, 30)?;
            mouse(
                &mut app,
                at(&buffer, "Task 38")?,
                MouseEventKind::Down(MouseButton::Left),
            )?;
            let Screen::Watch(viewer) = &app.screen else {
                anyhow::bail!("Expected viewer")
            };
            assert_eq!(text(&viewer.records[viewer.selection()], "name"), "Task 38");
            assert!(!viewer.follow);
            assert!(viewer.follow_logs);
            let buffer = render(&mut app, width, 30)?;
            let title = at(&buffer, "Task logs")?;
            mouse(
                &mut app,
                Position::new(title.x, title.y + 1),
                MouseEventKind::ScrollUp,
            )?;
            assert_eq!(app.pane_focus, 1);
            let Screen::Watch(viewer) = &app.screen else {
                anyhow::bail!("Expected viewer")
            };
            assert_eq!(text(&viewer.records[viewer.selection()], "name"), "Task 38");
            assert!(!viewer.follow_logs);
        }
        Ok(())
    }

    #[test]
    fn double_click_opens_selected_run_and_resize_discards_old_coordinates() -> Result<()> {
        let (_root, mut app) = super::super::tests::fixture()?;
        app.screen = Screen::Runs {
            pipeline: 1,
            values: vec![json!({"id":42,"name":"chosen-run"})],
        };
        let buffer = render(&mut app, 120, 30)?;
        let point = at(&buffer, "chosen-run")?;
        mouse(&mut app, point, MouseEventKind::Down(MouseButton::Left))?;
        assert!(matches!(app.screen, Screen::Runs { .. }));
        mouse(&mut app, point, MouseEventKind::Down(MouseButton::Left))?;
        assert!(matches!(&app.screen, Screen::Watch(viewer) if viewer.run_id == 42));
        app.handle(Event::Resize(50, 20))?;
        assert!(app.mouse.regions.is_empty());
        Ok(())
    }
}
