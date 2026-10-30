use super::tests::fixture;
use super::*;
use serde_json::json;

fn key(code: KeyCode) -> Event {
    Event::Key(event::KeyEvent::new(code, KeyModifiers::NONE))
}

fn render(app: &mut App, width: u16, height: u16) -> Result<Buffer> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
    terminal.draw(|f| draw(f, app))?;
    Ok(terminal.backend().buffer().clone())
}

fn contents(buffer: &Buffer) -> String {
    buffer.content.iter().map(|c| c.symbol()).collect()
}

#[test]
fn review_tabs_files_and_diffs_remain_usable_at_both_widths() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Pr {
        value: json!({"pullRequestId":7,"status":"active","detailChanges":{
            "iteration":2,"files":[
                {"changeType":"edit","item":{"path":"/first.rs"},"diff":"@@ -1 +1 @@\n-old\n+new\n"},
                {"changeType":"add","item":{"path":"/second.rs"},"diff":"+second"}
            ]
        }}),
    };
    app.handle(key(KeyCode::F(8)))?;
    assert_eq!(app.pr_tab, 1);
    app.handle(key(KeyCode::F(8)))?;
    assert_eq!(app.pr_tab, 2);
    app.handle(key(KeyCode::F(8)))?;
    assert_eq!(app.pr_tab, 3);
    assert_eq!(app.pane_focus, 0);
    for width in [70, 120] {
        app.pane_focus = 0;
        let buffer = render(&mut app, width, 30)?;
        let rendered = contents(&buffer);
        assert!(rendered.contains("F7/F8"));
        assert!(rendered.contains("first.rs"));
        assert!(rendered.contains("+new"));
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.symbol() == "+" && cell.fg == Color::Green)
        );
        app.handle(key(KeyCode::Enter))?;
        assert_eq!(app.pane_focus, 1);
        app.handle(key(KeyCode::Down))?;
        assert_eq!(app.pane_y[1], 1);
        app.handle(key(KeyCode::Char('w')))?;
        assert_eq!(app.pane_focus, 0);
        app.handle(key(KeyCode::Down))?;
        let Screen::Pr { value } = &app.screen else {
            anyhow::bail!("Expected PR");
        };
        assert_eq!(review::selected(value), 1);
        assert!(contents(&render(&mut app, width, 30)?).contains("+second"));
        app.handle(key(KeyCode::Up))?;
    }
    app.handle(key(KeyCode::F(8)))?;
    assert_eq!(app.pr_tab, 0);
    app.handle(key(KeyCode::Char('d')))?;
    assert_eq!(app.pr_tab, 3);
    app.handle(key(KeyCode::Char('v')))?;
    assert!(matches!(app.confirm, Some(PrCommand::Review { id: 7, .. })));
    Ok(())
}

#[test]
fn diff_replies_stay_with_their_pr_and_file_and_errors_leave_review_available() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Pr {
        value: json!({"pullRequestId":7,"detailChangeIndex":1,
        "detailChanges":{"files":[{"item":{"path":"/one"}},{"item":{"path":"/two"}}]}}),
    };
    app.pr_tab = 3;
    app.receive((
        app.generation,
        Message::PrDiff {
            id: 7,
            index: 0,
            diff: "first file".into(),
        },
    ));
    assert_eq!(app.pane_focus, 0);
    let Screen::Pr { value } = &app.screen else {
        anyhow::bail!("Expected PR");
    };
    assert_eq!(value["detailChanges"]["files"][0]["diff"], "first file");
    app.receive((
        app.generation,
        Message::PrDiff {
            id: 8,
            index: 1,
            diff: "wrong PR".into(),
        },
    ));
    app.receive((
        app.generation.wrapping_sub(1),
        Message::PrDiff {
            id: 7,
            index: 1,
            diff: "stale diff".into(),
        },
    ));
    app.receive((app.generation, Message::Error("Diff unavailable".into())));
    let Screen::Pr { value } = &app.screen else {
        anyhow::bail!("Expected PR");
    };
    assert!(value["detailChanges"]["files"][1]["diff"].is_null());
    assert_eq!(review::selected(value), 1);
    assert_eq!(app.status, "Diff unavailable");
    app.handle(key(KeyCode::F(7)))?;
    assert_eq!(app.pr_tab, 2);
    Ok(())
}

#[test]
fn editor_shortcut_works_for_repos_and_prs_and_keeps_invalid_path_for_retry() -> Result<()> {
    let (_root, mut app) = fixture()?;
    let repo = json!({"name":"demo","id":"repo"});
    let pr = json!({"pullRequestId":7,"repository":repo});
    for screen in [
        Screen::Repos(vec![repo.clone()]),
        Screen::Prs(vec![pr.clone()]),
        Screen::Pr { value: pr },
    ] {
        app.set_screen(screen);
        let hints = command_hints(&app);
        assert!(
            hints
                .iter()
                .any(|(hint, enabled)| hint == "Browser [b]" && *enabled)
        );
        app.handle(key(KeyCode::Char('o')))?;
        let form = app.form.as_mut().context("Editor form")?;
        assert!(matches!(form.kind, FormKind::OpenEditor(_)));
        form.fields[0].value = app
            .store
            .root
            .join("missing-checkout")
            .display()
            .to_string();
        assert!(app.submit().is_err());
        assert!(app.form.is_some());
        app.handle(key(KeyCode::Esc))?;
        assert!(app.form.is_none());
    }
    app.set_screen(Screen::Repos(vec![]));
    assert!(app.handle(key(KeyCode::Char('b'))).is_err());
    assert!(app.handle(key(KeyCode::Char('o'))).is_err());
    Ok(())
}

#[tokio::test]
async fn three_sections_cycle_in_order_and_number_keys_switch_during_reads() -> Result<()> {
    let (_root, mut app) = fixture()?;
    auth::pat::save(&app.store, "saved", "fixture-pat")?;
    let target = app.settings.target(None, None)?;
    app.pr_cache = Some((target.clone(), false, vec![]));
    app.repo_cache = Some((target, vec![json!({"id":"repo", "name":"Example"})]));
    app.screen = Screen::Recent(vec![]);
    for section in [1, 2, 0] {
        app.handle(key(KeyCode::Tab))?;
        assert_eq!(section_index(&app.screen), Some(section));
    }
    for section in [2, 1, 0] {
        app.handle(key(KeyCode::BackTab))?;
        assert_eq!(section_index(&app.screen), Some(section));
    }
    let generation = app.generation;
    app.handle(key(KeyCode::Char('3')))?;
    assert!(matches!(app.screen, Screen::Repos(_)));
    app.receive((generation, Message::Recent(vec![])));
    assert!(matches!(app.screen, Screen::Repos(_)));
    app.launch(std::future::pending());
    app.writing = true;
    app.handle(key(KeyCode::Char('1')))?;
    assert!(matches!(app.screen, Screen::Repos(_)));
    Ok(())
}

#[tokio::test]
async fn repositories_preserve_selection_refresh_and_respect_project_cache_scope() -> Result<()> {
    let (_root, mut app) = fixture()?;
    auth::pat::save(&app.store, "saved", "fixture-pat")?;
    app.screen = Screen::Repos(vec![]);
    app.receive((
        app.generation,
        Message::Repos(vec![
            json!({"id":"b","name":"Beta"}),
            json!({"id":"a","name":"Alpha"}),
        ]),
    ));
    app.handle(key(KeyCode::Down))?;
    assert_eq!(app.selection.selected(), Some(1));
    app.set_screen(Screen::Home);
    app.handle(key(KeyCode::Char('3')))?;
    assert_eq!(app.selection.selected(), Some(1));
    assert!(app.task.is_none());
    app.refresh()?;
    assert!(app.repo_cache.is_none());
    assert!(matches!(&app.screen, Screen::Repos(items) if items.len() == 2));
    app.cancel_task();
    app.receive((
        app.generation,
        Message::Repos(vec![
            json!({"id":"b","name":"Beta"}),
            json!({"id":"c","name":"Before"}),
            json!({"id":"a","name":"Alpha"}),
        ]),
    ));
    assert_eq!(app.selection.selected(), Some(2));
    app.settings.project = Some("different-project".into());
    app.set_screen(Screen::Home);
    app.load_repos()?;
    assert!(app.task.is_some());
    assert!(matches!(&app.screen, Screen::Repos(items) if items.is_empty()));
    Ok(())
}

#[test]
fn repositories_render_three_tabs_and_navigate_details_at_both_widths() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Repos(vec![
        json!({"id":"repo-id","name":"devops-cli","defaultBranch":"refs/heads/main","remoteUrl":"https://example.test/devops-cli","sshUrl":"git@example.test:devops-cli","webUrl":"https://example.test/browse"}),
    ]);
    for width in [40, 80, 120] {
        let buffer = render(&mut app, width, 30)?;
        let rendered = contents(&buffer);
        assert!(rendered.contains(if width < 60 {
            "Repos [3]"
        } else {
            "Repositories [3]"
        }));
        assert!(rendered.contains("Default branch: main"));
        assert!(rendered.contains("https://example.test/devops-cli"));
        assert!(!buffer[(0, 0)].modifier.contains(Modifier::UNDERLINED));
        let header: String = (0..width).map(|x| buffer[(x, 0)].symbol()).collect();
        let repo_column = header
            .find(if width < 60 {
                "Repos [3]"
            } else {
                "Repositories [3]"
            })
            .context("Third tab")? as u16;
        assert!(
            buffer[(repo_column, 0)]
                .modifier
                .contains(Modifier::UNDERLINED)
        );
        capture(&buffer, &format!("repos-{width}"))?;
        app.handle(key(KeyCode::Enter))?;
        assert_eq!(app.pane_focus, 1);
        app.handle(key(KeyCode::Down))?;
        assert_eq!(app.selection.selected(), Some(0));
        app.handle(key(KeyCode::Left))?;
        assert_eq!(app.pane_focus, 0);
        app.detail_scroll = 0;
    }
    Ok(())
}

#[test]
fn pane_keys_preserve_scroll_and_resize_never_focuses_a_hidden_pane() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Prs(vec![
        json!({"pullRequestId": 1, "description": "line\n".repeat(50)}),
    ]);
    app.handle(key(KeyCode::Right))?;
    app.handle(key(KeyCode::PageDown))?;
    assert_eq!(app.detail_scroll, 20);
    assert_eq!(app.selection.selected(), Some(0));
    app.handle(key(KeyCode::Left))?;
    assert_eq!(app.pane_focus, 0);
    app.handle(key(KeyCode::Char('w')))?;
    assert_eq!(app.pane_focus, 1);
    assert_eq!(app.detail_scroll, 20);
    render(&mut app, 70, 24)?;
    assert_eq!(app.pane_focus, 0);
    app.handle(key(KeyCode::F(6)))?;
    assert_eq!(app.pane_focus, 0);
    render(&mut app, 120, 30)?;
    app.handle(key(KeyCode::Right))?;
    assert_eq!(app.pane_focus, 1);
    Ok(())
}

#[test]
fn run_list_remains_available_and_escape_restores_its_selection() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Runs {
        pipeline: 7,
        values: vec![json!({"id":41}), json!({"id":42})],
    };
    app.enter()?;
    assert!(matches!(app.screen, Screen::Watch(_)));
    app.handle(key(KeyCode::Char('w')))?;
    app.handle(key(KeyCode::Char('w')))?;
    assert_eq!(app.pane_focus, 2);
    app.handle(key(KeyCode::Down))?;
    app.handle(key(KeyCode::Enter))?;
    assert!(app.run_list.is_some());
    let Screen::Watch(viewer) = &app.screen else {
        anyhow::bail!("Expected viewer")
    };
    assert_eq!(viewer.run_id, 42);
    app.handle(key(KeyCode::Esc))?;
    assert!(matches!(app.screen, Screen::Runs { pipeline: 7, .. }));
    assert_eq!(app.selection.selected(), Some(1));
    assert!(app.task.is_none());
    Ok(())
}

#[test]
fn project_picker_cancel_restores_the_workspace_without_saving() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Prs(vec![json!({"pullRequestId":1,"title":"Keep this context"})]);
    app.pane_focus = 1;
    app.detail_scroll = 3;
    app.setup_org = app.settings.organization.clone();
    let settings = app.settings.clone();
    app.receive((
        app.generation,
        Message::Projects(vec![json!({"name":"project"})]),
    ));
    let rendered = contents(&render(&mut app, 120, 30)?);
    assert!(rendered.contains("Keep this context"));
    assert!(rendered.contains("Current project"));
    assert!(rendered.contains("* project"));
    app.handle(key(KeyCode::Char('1')))?;
    app.handle(key(KeyCode::Tab))?;
    assert!(matches!(app.screen, Screen::Projects(_)));
    app.handle(key(KeyCode::Esc))?;
    assert!(matches!(app.screen, Screen::Prs(_)));
    assert_eq!(app.pane_focus, 1);
    assert_eq!(app.detail_scroll, 3);
    assert_eq!(app.settings, settings);
    assert!(app.picker_return.is_none());
    assert!(!app.store.root.exists());
    Ok(())
}

#[test]
fn multiline_forms_insert_lines_and_keep_input_after_a_failed_submission() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Pr {
        value: json!({"pullRequestId":7}),
    };
    app.open_pr_form('c')?;
    app.handle(Event::Paste("First line".into()))?;
    app.handle(key(KeyCode::Enter))?;
    app.handle(Event::Paste("Second line".into()))?;
    assert_eq!(
        app.form.as_ref().context("Expected comment")?.value(0)?,
        "First line\nSecond line"
    );
    assert!(app.task.is_none());
    // Missing credentials must not discard the draft.
    assert!(
        app.handle(Event::Key(event::KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL
        )))
        .is_err()
    );
    assert_eq!(
        app.form.as_ref().context("Keep draft")?.value(0)?,
        "First line\nSecond line"
    );
    let rendered = contents(&render(&mut app, 80, 24)?);
    assert!(rendered.contains("Save [Ctrl+S]"));
    assert!(rendered.contains("Cancel [Esc]"));
    assert!(rendered.contains("Second line"));
    app.handle(key(KeyCode::Esc))?;
    assert!(app.form.is_none());
    Ok(())
}

#[test]
fn help_is_modal_and_more_commands_does_not_change_selection() -> Result<()> {
    let (_root, mut app) = fixture()?;
    app.screen = Screen::Prs(vec![json!({"pullRequestId":1}), json!({"pullRequestId":2})]);
    app.handle(key(KeyCode::Char('.')))?;
    assert!(app.expanded_commands);
    let rendered = contents(&render(&mut app, 90, 30)?);
    assert!(rendered.contains("less [.]"));
    assert!(rendered.contains("Help [h]"));
    app.handle(key(KeyCode::Char('h')))?;
    app.handle(key(KeyCode::Down))?;
    app.handle(key(KeyCode::Enter))?;
    assert_eq!(app.selection.selected(), Some(0));
    assert!(app.task.is_none());
    assert!(contents(&render(&mut app, 90, 30)?).contains("Keyboard shortcuts"));
    app.handle(key(KeyCode::Esc))?;
    assert!(app.help_scroll.is_none());
    app.handle(key(KeyCode::Down))?;
    assert_eq!(app.selection.selected(), Some(1));
    Ok(())
}

#[test]
fn layouts_and_dialogs_render_at_small_and_wide_terminal_sizes() -> Result<()> {
    let (_root, mut app) = fixture()?;
    let pr = json!({"pullRequestId":842,"title":"Fix token refresh","status":"active","description":"Retry after refreshing an expired token.\nKeep the selected project.","sourceRefName":"refs/heads/feature/auth","targetRefName":"refs/heads/main","repository":{"name":"devops-cli"},"createdBy":{"displayName":"Alex"},"reviewers":[{"displayName":"Sam","vote":10}]});
    for (width, height) in [(120, 30), (80, 24), (40, 12), (20, 6), (1, 1)] {
        app.screen = Screen::Prs(vec![pr.clone()]);
        app.status.clear();
        app.form = None;
        app.help_scroll = None;
        app.expanded_commands = false;
        let buffer = render(&mut app, width, height)?;
        if width >= 80 {
            assert!(contents(&buffer).contains("Pull requests [2]"));
            assert!(contents(&buffer).contains("more [.]"));
            assert_eq!(buffer[(0, 2)].symbol(), "┌");
        }
        capture(&buffer, &format!("prs-{width}"))?;
        app.screen = Screen::Pr { value: pr.clone() };
        capture(
            &render(&mut app, width, height)?,
            &format!("details-{width}"),
        )?;
        app.open_pr_form('e')?;
        capture(&render(&mut app, width, height)?, &format!("edit-{width}"))?;
        app.form = None;
        app.handle(key(KeyCode::Char('h')))?;
        render(&mut app, width, height)?;
        app.help_scroll = None;
        let mut viewer = Viewer::new(425);
        viewer.run = json!({"id":425,"buildNumber":"20261030.1","status":"completed","result":"succeeded","definition":{"name":"Build and test"},"sourceBranch":"refs/heads/main","startTime":"2026-10-30T09:00:00Z","finishTime":"2026-10-30T09:02:14Z"});
        viewer.records = vec![
            json!({"id":"task","name":"Test","type":"Task","state":"completed","result":"succeeded"}),
        ];
        viewer.log.append(
            0,
            vec!["$ cargo test --locked".into(), "All tests passed.".into()],
        );
        app.screen = Screen::Watch(Box::new(viewer));
        app.run_list = Some(RunList {
            pipeline: None,
            items: vec![json!({"id":425,"name":"Build and test","result":"succeeded"})],
            selection: ListState::default().with_selected(Some(0)),
        });
        let buffer = render(&mut app, width, height)?;
        if width >= 90 {
            assert!(contents(&buffer).contains("Recent runs"));
            assert!(!contents(&buffer).contains("Pipeline:"));
            let logs = viewer_areas(Rect::new(0, 2, width, height - 3))[1];
            assert_eq!(buffer[(logs.x, 2)].symbol(), "┌");
            assert_eq!(buffer[(logs.x, height - 2)].symbol(), "└");
        }
        capture(&buffer, &format!("logs-{width}"))?;
    }
    Ok(())
}

// Optional fixture export for inspecting the actual Ratatui output without credentials.
fn capture(buffer: &Buffer, name: &str) -> Result<()> {
    if let Ok(directory) = std::env::var("DEVOPS_TUI_CAPTURE_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        let cells: Vec<_> = buffer.content.iter().map(|c| json!({"text":c.symbol(),"fg":format!("{:?}",c.fg),"bg":format!("{:?}",c.bg),"bold":c.modifier.contains(Modifier::BOLD),"underline":c.modifier.contains(Modifier::UNDERLINED)})).collect();
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(
                &json!({"width":buffer.area.width,"height":buffer.area.height,"cells":cells}),
            )?,
        )?;
    }
    Ok(())
}
