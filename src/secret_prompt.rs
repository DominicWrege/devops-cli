use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::io::Write;

struct RawMode;
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stderr(), event::DisableBracketedPaste);
        let _ = disable_raw_mode();
    }
}

enum Action {
    Continue,
    Submit,
    Cancel,
}

fn edit(value: &mut String, event: Event) -> Action {
    match event {
        Event::Paste(text) => value.extend(text.chars().filter(|c| !c.is_control())),
        Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
            KeyCode::Esc => return Action::Cancel,
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::Cancel;
            }
            KeyCode::Enter => return Action::Submit,
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !c.is_control() =>
            {
                value.push(c)
            }
            _ => {}
        },
        _ => {}
    }
    Action::Continue
}

pub fn read(label: &str) -> Result<String> {
    let mut output = std::io::stderr();
    write!(output, "{label}")?;
    output.flush()?;
    enable_raw_mode()?;
    let guard = RawMode;
    crossterm::execute!(output, event::EnableBracketedPaste)?;
    let mut value = String::new();
    let result = (|| -> Result<String> {
        loop {
            let before = value.chars().count();
            let action = edit(&mut value, event::read()?);
            let after = value.chars().count();
            if after > before {
                write!(output, "{}", "*".repeat(after - before))?;
            } else if after < before {
                write!(output, "{}", "\x08 \x08".repeat(before - after))?;
            }
            output.flush()?;
            match action {
                Action::Submit => return Ok(std::mem::take(&mut value)),
                Action::Cancel => anyhow::bail!("Token entry canceled"),
                Action::Continue => {}
            }
        }
    })();
    drop(guard);
    writeln!(output)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    #[test]
    fn paste_edit_and_cancel() {
        let mut value = String::new();
        edit(&mut value, Event::Paste("secret".into()));
        edit(
            &mut value,
            Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        );
        assert_eq!(value, "secre");
        assert!(matches!(
            edit(
                &mut value,
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            ),
            Action::Submit
        ));
        assert!(matches!(
            edit(
                &mut value,
                Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            ),
            Action::Cancel
        ));
    }
}
