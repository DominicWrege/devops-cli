use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

pub(super) fn pane_block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::bordered()
        .style(Style::default().fg(Color::Gray).bg(Color::Black))
        .title(title)
        .border_style(Style::default().fg(if focused {
            Color::Gray
        } else {
            Color::DarkGray
        }))
        .title_style(Style::default().fg(if focused {
            Color::White
        } else {
            Color::DarkGray
        }))
}

pub(super) fn selection_style() -> Style {
    Style::default().fg(Color::White).bg(Color::Blue)
}

pub(super) fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

pub(super) fn draw_header(frame: &mut Frame, area: Rect, section: Option<usize>, context: &str) {
    let tab = |label, active| {
        Span::styled(
            label,
            if active {
                Style::default().fg(Color::White).underlined()
            } else {
                Style::default().fg(Color::DarkGray)
            },
        )
    };
    let labels = if area.width >= 60 {
        ["Pipelines [1]", "Pull requests [2]", "Repositories [3]"]
    } else {
        ["Pipes [1]", "PRs [2]", "Repos [3]"]
    };
    let mut spans = Vec::new();
    for (index, label) in labels.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" | "));
        }
        spans.push(tab(label, section == Some(index)));
    }
    let tabs = Line::from(spans);
    let width = (tabs.width() as u16).min(area.width);
    frame.render_widget(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
        area,
    );
    frame.render_widget(
        Paragraph::new(tabs),
        Rect::new(area.x, area.y, width, area.height.min(1)),
    );
    let remaining = area.width.saturating_sub(width + 2);
    if remaining > 0 {
        let mut context = crate::output::cell(context);
        if Line::raw(&context).width() > remaining as usize {
            let mut tail = String::new();
            for ch in context.chars().rev() {
                let next = format!("{ch}{tail}");
                if Line::raw(&next).width() + 1 > remaining as usize {
                    break;
                }
                tail = next;
            }
            context = format!("…{tail}");
        }
        frame.render_widget(
            Paragraph::new(context)
                .right_aligned()
                .style(Style::default().fg(Color::DarkGray)),
            Rect::new(
                area.right() - remaining,
                area.y,
                remaining,
                area.height.min(1),
            ),
        );
    }
}

fn command_lines(
    commands: &[(String, bool)],
    width: u16,
    expanded: bool,
    expandable: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut line = Line::default();
    // Leave space for the expansion shortcut even in the collapsed bar.
    let available = if expanded || !expandable {
        width
    } else {
        width.saturating_sub(9)
    } as usize;
    for (command, enabled) in commands {
        let span = Span::styled(
            command.clone(),
            if *enabled {
                selection_style()
            } else {
                Style::default().fg(Color::DarkGray).bg(Color::Blue)
            },
        );
        if line.width() + span.width() > available && !line.spans.is_empty() {
            lines.push(line);
            if !expanded && expandable {
                return lines;
            }
            line = Line::default();
        }
        line.spans.push(span);
        line.spans.push(Span::raw(" "));
    }
    lines.push(line);
    if expanded && expandable {
        lines.push(Line::default());
    }
    lines
}

pub(super) fn command_rows(
    commands: &[(String, bool)],
    width: u16,
    expanded: bool,
    expandable: bool,
) -> u16 {
    command_lines(commands, width, expanded, expandable).len() as u16
}

pub(super) fn draw_commands(
    frame: &mut Frame,
    area: Rect,
    commands: &[(String, bool)],
    expanded: bool,
    expandable: bool,
) {
    frame.render_widget(
        Paragraph::new(command_lines(commands, area.width, expanded, expandable)),
        area,
    );
    if expandable && area.height > 0 {
        let label = if expanded { "less [.]" } else { "more [.]" };
        let width = area.width.min(8);
        frame.render_widget(
            Paragraph::new(label).right_aligned(),
            Rect::new(area.right() - width, area.bottom() - 1, width, 1),
        );
    }
}
