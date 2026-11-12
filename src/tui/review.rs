use super::*;

pub(super) fn selected(pr: &Value) -> usize {
    pr["detailChangeIndex"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0)
}

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    pr: &Value,
    focus: usize,
    scroll: [u16; 3],
    horizontal: [u16; 3],
) {
    let changes = &pr["detailChanges"];
    let rows = pr_rows(&changes["files"]);
    let layout = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
    let header = if let Some(label) = pr["detailLabel"].as_str() {
        Text::raw(format!(
            "{}\nEnter: load diff · w: files/diff · Esc: history",
            output::cell(label)
        ))
    } else {
        Text::from(vec![
            pr_tabs(3, area.width, pr["status"] == "active"),
            Line::raw(format!(
                "PR #{} · Iteration {}",
                scalar(&pr["pullRequestId"]),
                scalar(&changes["iteration"])
            )),
        ])
    };
    frame.render_widget(Paragraph::new(header), layout[0]);
    let panes = if area.width >= 90 {
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(layout[1])
    } else {
        Layout::vertical([Constraint::Percentage(35), Constraint::Percentage(65)]).split(layout[1])
    };
    let index = selected(pr);
    if rows.is_empty() {
        let message = if changes.is_null() {
            "Loading changed files. Press F5 to retry."
        } else {
            "No changed files."
        };
        frame.render_widget(
            Paragraph::new(message)
                .wrap(Wrap { trim: false })
                .block(pane_block("Changed files", focus == 0)),
            panes[0],
        );
    } else {
        let items: Vec<ListItem> = rows
            .iter()
            .map(|file| {
                let path = text(&file["item"], "path");
                let old = text(file, "originalPath");
                let name = if old.is_empty() {
                    path.to_string()
                } else {
                    format!("{old} → {path}")
                };
                ListItem::new(output::clean(&format!(
                    "{}  {name}",
                    text(file, "changeType")
                )))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(index));
        frame.render_stateful_widget(
            List::new(items)
                .block(pane_block(
                    if pr["detailLabel"].is_string() {
                        "Changed files · Enter: diff"
                    } else {
                        "Changed files"
                    },
                    focus == 0,
                ))
                .highlight_style(selection_style())
                .highlight_symbol("› "),
            panes[0],
            &mut state,
        );
    }
    let file = rows.get(index);
    let diff = file
        .and_then(|v| v["diff"].as_str())
        .unwrap_or(if pr["detailLabel"].is_string() {
            "Select a file and press Enter to load its diff."
        } else if file.is_some() {
            "Loading file diff..."
        } else {
            "No file selected."
        });
    let lines: Vec<Line> = output::clean(diff)
        .lines()
        .map(|line| {
            let color = if line.starts_with('+') {
                Color::Green
            } else if line.starts_with('-') {
                Color::Red
            } else if line.starts_with("@@") {
                Color::Cyan
            } else {
                Color::Gray
            };
            Line::styled(line.to_owned(), Style::default().fg(color))
        })
        .collect();
    let title = file
        .map(|v| format!("Diff · {}", output::clean(text(&v["item"], "path"))))
        .unwrap_or_else(|| "Diff".into());
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll[1], horizontal[1]))
            .block(pane_block(title, focus == 1)),
        panes[1],
    );
}
