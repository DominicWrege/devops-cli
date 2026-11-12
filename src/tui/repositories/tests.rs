use super::*;
use serde_json::json;

fn key(code: KeyCode) -> Event {
    Event::Key(event::KeyEvent::new(code, KeyModifiers::NONE))
}

fn tag(name: &str, sha: &str, date: &str) -> Value {
    json!({"name":format!("refs/tags/{name}"),"objectId":sha,
        "targetCommit":{"commitId":sha,"comment":format!("Release {name}"),"committer":{"date":date}}})
}

fn setup(app: &mut App) -> Result<String> {
    app.screen = Screen::Repos(vec![
        json!({"id":"repo","name":"demo","defaultBranch":"refs/heads/main"}),
    ]);
    app.repository_view_mut()?;
    let (key, _) = app.repository()?;
    app.receive_repository(key.clone(), Reply::Commits { branch:"main".into(), skip:0, values:vec![
        json!({"commitId":"new","comment":"Newest change","author":{"name":"Dominic"},"committer":{"date":"2026-11-02T09:00:00Z"}}),
        json!({"commitId":"old","comment":"Older change","author":{"name":"Dominic"},"committer":{"date":"2026-11-01T09:00:00Z"}}),
    ]});
    app.receive_repository(
        key.clone(),
        Reply::Refs {
            branches: false,
            values: vec![
                tag("fix/2.6.0", "old", "2026-11-01T09:00:00Z"),
                tag("fix/2.6.1", "new", "2026-11-02T09:00:00Z"),
                tag("release/2.6.0", "old", "2026-11-01T09:00:00Z"),
            ],
        },
    );
    Ok(key)
}

fn setup_filter(app: &mut App) {
    app.screen = Screen::Repos(vec![]);
    app.receive((
        app.generation,
        Message::Repos(vec![
            json!({"id":"a","name":"api-core"}),
            json!({"id":"b","name":"API-web"}),
            json!({"id":"c","name":"web-app"}),
        ]),
    ));
}

fn setup_tag_actions(app: &mut App) -> Result<String> {
    let id = setup(app)?;
    let view = app.repo_views.get_mut(&id).context("View")?;
    for commit in &mut view.histories.get_mut("main").context("History")?.commits {
        commit["commitId"] = json!(if commit["commitId"] == "new" {
            "1".repeat(40)
        } else {
            "2".repeat(40)
        });
    }
    for tag in view.tags.as_mut().context("Tags")? {
        let sha = if tag["objectId"] == "new" {
            "1".repeat(40)
        } else {
            "2".repeat(40)
        };
        tag["objectId"] = json!(sha);
        tag["targetCommit"]["commitId"] = json!(sha);
        if sha == "1".repeat(40) {
            tag["objectId"] = json!("3".repeat(40));
            tag["peeledObjectId"] = json!(sha);
        }
    }
    app.handle(key(KeyCode::Char('t')))?;
    Ok(id)
}

#[test]
fn tag_forms_use_selected_commits_and_validate_names_targets_and_duplicates() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    setup_tag_actions(&mut app)?;
    for tab in ['c', 't'] {
        app.handle(key(KeyCode::Char(tab)))?;
        app.handle(key(KeyCode::Char('n')))?;
        let form = app.form.as_ref().context("Create form")?;
        assert!(matches!(&form.kind, FormKind::Tag(edit) if edit.original.is_none()));
        assert_eq!(form.value(1)?, "1".repeat(40));
        assert!(form.value(0)?.is_empty());
        app.handle(Event::Paste("bad..name".into()))?;
        assert!(app.submit().is_err());
        app.form.as_mut().context("Form")?.fields[0].value = "new-tag".into();
        app.form.as_mut().context("Form")?.fields[1].value = "short".into();
        assert!(app.submit().is_err());
        assert!(app.task.is_none());
        app.handle(key(KeyCode::Esc))?;
    }
    app.handle(key(KeyCode::Char('e')))?;
    let form = app.form.as_ref().context("Rename form")?;
    assert_eq!(form.value(0)?, "fix/2.6.1");
    assert!(
        matches!(&form.kind, FormKind::Tag(edit) if edit.original.as_ref().is_some_and(|tag| tag["objectId"] == "3".repeat(40)))
    );
    assert!(
        app.submit()
            .err()
            .context("no-op rename")?
            .to_string()
            .contains("different")
    );
    app.form.as_mut().context("Form")?.fields[0].value = "fix/2.6.0".into();
    assert!(
        app.submit()
            .err()
            .context("duplicate")?
            .to_string()
            .contains("already exists")
    );
    assert!(app.form.is_some());
    assert!(app.task.is_none());
    app.handle(key(KeyCode::Esc))?;
    app.handle(key(KeyCode::Left))?; // folder, not a tag
    assert!(
        command_hints(&app)
            .iter()
            .filter(|(hint, _)| matches!(hint.as_str(), "Rename [e]" | "Delete [d/Del]"))
            .all(|(_, enabled)| !enabled)
    );
    for code in [KeyCode::Char('e'), KeyCode::Char('d'), KeyCode::Delete] {
        assert!(app.handle(key(code)).is_err());
        assert!(app.form.is_none() && app.tag_confirm.is_none() && app.task.is_none());
    }
    Ok(())
}

#[test]
fn new_tags_use_the_commit_selection_instead_of_an_existing_tag_target() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    let id = setup_tag_actions(&mut app)?;
    let newest = format!("bee53e3{}", "0".repeat(33));
    let older = format!("2823f99{}", "0".repeat(33));
    {
        let view = app.repo_views.get_mut(&id).context("View")?;
        let history = view.histories.get_mut("main").context("History")?;
        history.commits[0]["commitId"] = json!(newest);
        history.commits[1]["commitId"] = json!(older);
        view.tags = Some(vec![tag("2.1.0", &older, "2026-08-25T09:00:00Z")]);
        view.tags_selection.select(Some(0));
    }
    for (selection, expected) in [(0, &newest), (1, &older)] {
        app.repo_views
            .get_mut(&id)
            .context("View")?
            .histories
            .get_mut("main")
            .context("History")?
            .selection
            .select(Some(selection));
        for tab in ['c', 't'] {
            app.handle(key(KeyCode::Char(tab)))?;
            app.handle(key(KeyCode::Char('n')))?;
            assert_eq!(
                app.form.as_ref().context("Create form")?.value(1)?,
                expected
            );
            app.handle(key(KeyCode::Esc))?;
        }
    }
    // Without loaded history, require an explicit SHA rather than copying a tag.
    app.repo_views
        .get_mut(&id)
        .context("View")?
        .histories
        .clear();
    app.handle(key(KeyCode::Char('n')))?;
    assert!(
        app.form
            .as_ref()
            .context("Create form")?
            .value(1)?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn tag_deletion_requires_confirmation_and_submitted_writes_block_navigation() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    auth::pat::save(&app.store, "saved", "fixture-pat")?;
    setup_tag_actions(&mut app)?;
    app.handle(key(KeyCode::Char('d')))?;
    assert!(
        matches!(&app.tag_confirm.as_ref().context("Confirmation")?.change,
        TagChange::Delete { name, object } if name == "refs/tags/fix/2.6.1" && object == &"3".repeat(40))
    );
    assert!(app.task.is_none());
    app.handle(key(KeyCode::Char('3')))?;
    assert!(app.tag_confirm.is_some());
    for (width, height) in [(15, 6), (40, 24), (120, 32)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
        terminal.draw(|frame| draw(frame, &mut app))?;
        if width >= 40 {
            let rendered: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(rendered.contains("Delete tag fix/2.6.1?"));
            assert!(rendered.contains("y: confirm"));
        }
    }
    app.handle(key(KeyCode::Char('n')))?;
    assert!(app.tag_confirm.is_none() && app.task.is_none());
    app.pane_focus = 1;
    app.handle(key(KeyCode::Delete))?;
    app.handle(key(KeyCode::Esc))?;
    assert!(app.tag_confirm.is_none() && app.task.is_none());
    app.handle(key(KeyCode::Delete))?;
    app.handle(key(KeyCode::Char('y')))?;
    let generation = app.generation;
    assert!(app.writing && app.task.is_some());
    for code in [
        KeyCode::Char('c'),
        KeyCode::F(5),
        KeyCode::Esc,
        KeyCode::Char('3'),
        KeyCode::Char('r'),
        KeyCode::Char('n'),
        KeyCode::Char('q'),
    ] {
        assert!(!app.handle(key(code))?);
        assert_eq!(app.generation, generation);
        assert!(app.writing && app.task.is_some());
    }
    app.cancel_task();
    app.receive((app.generation, Message::Error("staleOldObjectId".into())));
    assert!(!app.writing);
    assert!(app.tag_confirm.is_none());
    assert_eq!(
        app.repository_view()
            .context("View")?
            .tags
            .as_ref()
            .context("Tags")?
            .len(),
        3
    );
    Ok(())
}

#[tokio::test]
async fn failed_tag_saves_keep_form_values_and_allow_correction() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    auth::pat::save(&app.store, "saved", "fixture-pat")?;
    setup_tag_actions(&mut app)?;
    app.handle(key(KeyCode::Char('e')))?;
    app.form.as_mut().context("Form")?.fields[0].value = "release/new".into();
    app.handle(key(KeyCode::Enter))?;
    assert!(app.writing && app.task.is_some());
    app.handle(Event::Paste("cannot edit while saving".into()))?;
    app.handle(key(KeyCode::Esc))?;
    assert_eq!(app.form.as_ref().context("Form")?.value(0)?, "release/new");
    app.cancel_task();
    app.receive((app.generation, Message::Error("Permission denied".into())));
    assert!(!app.writing);
    assert_eq!(app.form.as_ref().context("Form")?.value(0)?, "release/new");
    assert!(app.status.contains("Permission denied"));
    for (width, height) in [(15, 6), (40, 24), (120, 32)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
        terminal.draw(|frame| draw(frame, &mut app))?;
        if width >= 40 {
            let rendered: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(rendered.contains("Rename tag"));
            assert!(rendered.contains("release/new"));
        }
    }
    app.handle(key(KeyCode::Esc))?;
    assert!(app.form.is_none());
    Ok(())
}

#[test]
fn saved_tags_select_and_reveal_the_destination_even_when_refresh_fails() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    let id = setup_tag_actions(&mut app)?;
    app.handle(key(KeyCode::Char('e')))?;
    app.repo_views
        .get_mut(&id)
        .context("View")?
        .collapsed_tags
        .insert("release/".into());
    app.receive((
        app.generation,
        Message::Repository {
            key: id.clone(),
            reply: Reply::TagWritten {
                change: TagChange::Rename {
                    old: "refs/tags/fix/2.6.1".into(),
                    name: "refs/tags/release/new".into(),
                    object: "3".repeat(40),
                },
                refreshed: Err("offline".into()),
            },
        },
    ));
    let view = app.repository_view().context("View")?;
    let selected =
        &view.tags.as_ref().context("Tags")?[view.selected_ref().context("Selected tag")?];
    assert_eq!(selected["name"], "refs/tags/release/new");
    assert_eq!(selected["objectId"], "3".repeat(40));
    assert_eq!(ref_commit(selected), "1".repeat(40));
    assert!(!view.collapsed_tags.contains("release/"));
    assert!(app.form.is_none());
    assert!(app.status.contains("Renamed tag fix/2.6.1 to release/new"));
    assert!(app.status.contains("refresh failed: offline"));
    app.repo_views
        .get_mut(&id)
        .context("View")?
        .collapsed_tags
        .insert("release/security/".into());
    let created = tag(
        "release/security/v3",
        &"1".repeat(40),
        "2026-11-02T08:00:00Z",
    );
    app.receive((
        app.generation,
        Message::Repository {
            key: id.clone(),
            reply: Reply::TagWritten {
                change: TagChange::Create {
                    name: "refs/tags/release/security/v3".into(),
                    target: "1".repeat(40),
                },
                refreshed: Ok(vec![created]),
            },
        },
    ));
    let view = app.repository_view().context("View")?;
    assert!(view.collapsed_tags.is_empty());
    assert_eq!(view.selected_ref(), Some(0));
    assert_eq!(view.selected_commit(), Some("1".repeat(40).as_str()));
    app.receive((
        app.generation,
        Message::Repository {
            key: id,
            reply: Reply::TagWritten {
                change: TagChange::Delete {
                    name: "refs/tags/release/security/v3".into(),
                    object: "1".repeat(40),
                },
                refreshed: Err("offline".into()),
            },
        },
    ));
    let view = app.repository_view().context("View")?;
    assert!(view.tags.as_ref().context("Tags")?.is_empty());
    assert!(view.tags_selection.selected().is_none());
    assert!(app.status.contains("Deleted tag release/security/v3"));
    assert!(
        command_hints(&app)
            .iter()
            .any(|(hint, enabled)| hint == "New tag [n]" && *enabled)
    );
    app.handle(key(KeyCode::Char('n')))?;
    assert!(app.form.is_some()); // First tag can also be created in an empty list.
    Ok(())
}

#[test]
fn repository_filter_preserves_selection_and_details_use_the_matching_repository() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    setup_filter(&mut app);
    app.handle(key(KeyCode::Down))?;
    app.handle(key(KeyCode::Char('/')))?;
    for ch in "API".chars() {
        assert!(!app.handle(key(KeyCode::Char(ch)))?);
    }
    assert!(matches!(&app.screen, Screen::Repos(items) if items.len() == 2));
    assert_eq!(app.repository()?.1["id"], "b");
    assert_eq!(app.repo_cache.as_ref().context("Cache")?.1.len(), 3);
    app.handle(key(KeyCode::Enter))?;
    assert!(!app.repo_filter.editing);
    app.handle(key(KeyCode::Char('i')))?;
    assert_eq!(app.repository_view().context("View")?.tab, 1);
    assert_eq!(app.external_selection()?.0["id"], "b");
    app.handle(key(KeyCode::Left))?;
    app.handle(key(KeyCode::Esc))?;
    assert!(app.repo_filter.query.is_empty());
    assert!(matches!(&app.screen, Screen::Repos(items) if items.len() == 3));
    assert_eq!(app.repository()?.1["id"], "b");
    assert!(app.task.is_none());
    assert!(
        !command_hints(&app)
            .iter()
            .any(|(hint, _)| hint == "Sections [Tab/1/2/3]")
    );
    Ok(())
}

#[test]
fn repository_filter_handles_empty_results_paste_and_clear_at_all_widths() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    setup_filter(&mut app);
    app.handle(key(KeyCode::Char('/')))?;
    for ch in "qhi".chars() {
        assert!(!app.handle(key(KeyCode::Char(ch)))?);
    }
    assert!(app.help_scroll.is_none());
    assert!(matches!(&app.screen, Screen::Repos(items) if items.is_empty()));
    assert!(app.repository().is_err());
    for (width, height) in [(15, 6), (40, 24), (120, 32)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
        terminal.draw(|frame| draw(frame, &mut app))?;
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("qhi"));
        if width >= 40 {
            assert!(rendered.contains("Filter: qhi"));
            assert!(rendered.contains("No repositories match"));
            assert!(rendered.contains("0 / 3"));
        }
    }
    app.handle(Event::Key(event::KeyEvent::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    )))?;
    app.handle(Event::Paste("API".into()))?;
    assert!(matches!(&app.screen, Screen::Repos(items) if items.len() == 2));
    app.handle(key(KeyCode::Backspace))?;
    assert_eq!(app.repo_filter.query, "AP");
    app.handle(key(KeyCode::Esc))?;
    assert!(!app.repo_filter.editing);
    assert!(matches!(&app.screen, Screen::Repos(items) if items.len() == 3));
    Ok(())
}

#[tokio::test]
async fn repository_filter_can_be_entered_while_the_initial_list_is_loading() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    app.screen = Screen::Repos(vec![]);
    app.launch(std::future::pending());
    let generation = app.generation;
    app.handle(key(KeyCode::Char('/')))?;
    app.handle(Event::Paste("API".into()))?;
    assert!(app.task.is_some());
    assert_eq!(app.generation, generation);
    app.receive((
        generation,
        Message::Repos(vec![
            json!({"id":"a","name":"api-core"}),
            json!({"id":"b","name":"web-app"}),
        ]),
    ));
    assert!(app.repo_filter.editing);
    assert!(matches!(&app.screen, Screen::Repos(items) if items.len() == 1));
    assert_eq!(app.repository()?.1["id"], "a");
    Ok(())
}

#[tokio::test]
async fn repository_filter_survives_refresh_and_sections_and_resets_for_another_project()
-> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    auth::pat::save(&app.store, "saved", "fixture-pat")?;
    setup_filter(&mut app);
    app.handle(key(KeyCode::Char('/')))?;
    app.handle(Event::Paste("api".into()))?;
    app.handle(key(KeyCode::Enter))?;
    app.handle(key(KeyCode::Down))?;
    app.handle(key(KeyCode::F(5)))?;
    assert!(app.task.is_some());
    app.cancel_task();
    app.receive((
        app.generation,
        Message::Repos(vec![
            json!({"id":"d","name":"api-new"}),
            json!({"id":"b","name":"API-web"}),
            json!({"id":"c","name":"web-app"}),
        ]),
    ));
    assert_eq!(app.repository()?.1["id"], "b");
    assert!(matches!(&app.screen, Screen::Repos(items) if items.len() == 2));
    app.set_screen(Screen::Home);
    app.load_repos()?;
    assert_eq!(app.repo_filter.query, "api");
    assert_eq!(app.repository()?.1["id"], "b");
    app.settings.project = Some("another".into());
    app.repo_cache = Some((
        app.settings.target(None, None)?,
        vec![json!({"id":"other","name":"Other"})],
    ));
    app.load_repos()?;
    assert!(app.repo_filter.query.is_empty());
    assert_eq!(app.repository()?.1["id"], "other");
    assert!(app.task.is_none());
    Ok(())
}

#[test]
fn nested_paths_group_tags_by_commit_time_with_root_and_unknown_dates() {
    let values = vec![
        tag("fix/2.6.1", "a", "2026-11-02T09:00:00+02:00"),
        tag("fix/2.6.10", "b", "2026-11-02T08:00:00Z"),
        tag("fix/unknown", "c", ""),
        tag("fix/security/1.0", "d", "2026-11-01T08:00:00Z"),
        tag("v2.4.0", "e", "2026-10-01T08:00:00Z"),
        tag("release/2.6.0", "f", "2026-11-01T08:00:00Z"),
    ];
    let rows = ref_rows(&values, false, &BTreeSet::new());
    let leaves: Vec<_> = rows.iter().filter_map(|r| r.index).collect();
    assert_eq!(leaves, [3, 1, 0, 2, 5, 4]);
    assert_eq!(rows[0].name, "fix/");
    assert_eq!(rows[0].count, 4);
    assert_eq!(rows[1].name, "security/");
    assert_eq!(rows[2].depth, 2);
    assert!(rows.iter().any(|r| r.name == "(root)"));
    let collapsed = BTreeSet::from(["fix/".into(), "".into()]);
    let rows = ref_rows(&values, false, &collapsed);
    assert_eq!(rows.iter().filter_map(|r| r.index).collect::<Vec<_>>(), [5]);
    assert_eq!(rows[0].count, 4);
}

#[test]
fn tabs_folder_keys_and_branch_selection_preserve_each_history() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    let id = setup(&mut app)?;
    assert_eq!(app.repository_view().context("View")?.tab, 0);
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("new")
    );
    app.handle(key(KeyCode::Char('c')))?;
    assert_eq!(app.pane_focus, 1);
    app.handle(key(KeyCode::Down))?;
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("old")
    );
    app.handle(key(KeyCode::Char('t')))?;
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("new")
    );
    app.handle(key(KeyCode::Left))?;
    app.handle(key(KeyCode::Enter))?;
    assert!(
        app.repository_view()
            .context("View")?
            .collapsed_tags
            .contains("fix/")
    );
    app.handle(key(KeyCode::Right))?;
    app.handle(key(KeyCode::Down))?;
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("new")
    );
    app.handle(key(KeyCode::F(7)))?;
    assert_eq!(app.repository_view().context("View")?.tab, 1);
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        None
    );
    app.handle(key(KeyCode::F(7)))?;
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("old")
    );

    app.receive_repository(
        id.clone(),
        Reply::Refs {
            branches: true,
            values: vec![
                json!({"name":"refs/heads/main","objectId":"new"}),
                json!({"name":"refs/heads/fix/2.6.1","objectId":"old"}),
            ],
        },
    );
    app.receive_repository(
        id,
        Reply::Commits {
            branch: "fix/2.6.1".into(),
            skip: 0,
            values: vec![json!({"commitId":"fix"})],
        },
    );
    app.handle(key(KeyCode::Char('r')))?;
    app.handle(key(KeyCode::Home))?;
    app.handle(key(KeyCode::Down))?;
    app.handle(key(KeyCode::Enter))?;
    let view = app.repository_view().context("View")?;
    assert_eq!(view.branch, "fix/2.6.1");
    assert!(!view.picking_branch);
    assert_eq!(view.selected_commit(), Some("fix"));
    assert_eq!(view.histories["main"].selection.selected(), Some(1));
    assert!(app.task.is_none());
    for (code, tab) in [
        (KeyCode::Char('i'), 1),
        (KeyCode::Char('t'), 2),
        (KeyCode::Char('['), 1),
        (KeyCode::Char(']'), 2),
        (KeyCode::F(8), 0),
        (KeyCode::F(7), 2),
    ] {
        app.handle(key(code))?;
        assert_eq!(app.repository_view().context("View")?.tab, tab);
    }
    Ok(())
}

#[tokio::test]
async fn refresh_keeps_rows_and_identity_and_ignores_canceled_replies() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    auth::pat::save(&app.store, "saved", "fixture-pat")?;
    let id = setup(&mut app)?;
    app.handle(key(KeyCode::Char('c')))?;
    app.handle(key(KeyCode::Down))?;
    app.handle(key(KeyCode::F(5)))?;
    let generation = app.generation;
    assert!(app.task.is_some());
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("old")
    );
    app.cancel_task();
    app.receive((
        generation,
        Message::Repository {
            key: id.clone(),
            reply: Reply::Commits {
                branch: "main".into(),
                skip: 0,
                values: vec![],
            },
        },
    ));
    assert_eq!(
        app.repository_view().context("View")?.histories["main"]
            .commits
            .len(),
        2
    );
    app.receive((
        app.generation,
        Message::Repository {
            key: id.clone(),
            reply: Reply::Commits {
                branch: "main".into(),
                skip: 0,
                values: vec![
                    json!({"commitId":"newer"}),
                    json!({"commitId":"new"}),
                    json!({"commitId":"old"}),
                ],
            },
        },
    ));
    assert_eq!(
        app.repository_view().context("View")?.histories["main"]
            .selection
            .selected(),
        Some(2)
    );
    app.receive_repository(
        id.clone(),
        Reply::Commits {
            branch: "main".into(),
            skip: 3,
            values: vec![json!({"commitId":"older"})],
        },
    );
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("old")
    );
    assert_eq!(
        app.repository_view().context("View")?.histories["main"]
            .commits
            .len(),
        4
    );
    app.receive_repository(
        id,
        Reply::Commits {
            branch: "main".into(),
            skip: 1,
            values: vec![json!({"commitId":"wrong-page"})],
        },
    );
    assert_eq!(
        app.repository_view().context("View")?.histories["main"]
            .commits
            .len(),
        4
    );
    app.settings.project = Some("another".into());
    assert!(app.repository_view().is_none());
    assert_eq!(app.repository_view_mut()?.tab, 0);
    app.settings.project = Some("project".into());
    assert_eq!(
        app.repository_view().context("View")?.selected_commit(),
        Some("old")
    );
    Ok(())
}

#[test]
fn commit_files_diffs_and_escape_stay_with_the_selected_commit() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    let id = setup(&mut app)?;
    app.handle(key(KeyCode::Char('c')))?;
    app.receive_repository(
        id.clone(),
        Reply::Changes(
            json!({"detailLabel":"Commit new", "detailChanges":{"head":"new","files":[
                {"item":{"path":"/first"},"diff":"-old\n+new"},
                {"item":{"path":"/second"},"diff":"+second"}
            ]}}),
        ),
    );
    app.handle(key(KeyCode::Down))?;
    app.receive_repository(
        id,
        Reply::Diff {
            commit: "another".into(),
            index: 1,
            diff: "wrong".into(),
        },
    );
    app.handle(key(KeyCode::Enter))?;
    assert_eq!(app.pane_focus, 1);
    let detail = app
        .repository_view()
        .context("View")?
        .detail
        .as_ref()
        .context("Detail")?;
    assert_eq!(detail["detailChanges"]["files"][1]["diff"], "+second");
    app.handle(key(KeyCode::Down))?;
    assert_eq!(app.pane_y[1], 1);
    app.handle(key(KeyCode::Home))?;
    assert_eq!(app.pane_y[1], 0);
    app.handle(key(KeyCode::Esc))?;
    assert!(app.repository_view().context("View")?.detail.is_none());
    assert_eq!(app.repository_view().context("View")?.tab, 0);
    assert_eq!(app.pane_focus, 1);
    app.handle(key(KeyCode::Esc))?;
    assert_eq!(app.pane_focus, 0);
    Ok(())
}

#[test]
fn history_tags_and_commit_diffs_render_at_narrow_and_wide_sizes() -> Result<()> {
    let (_root, mut app) = super::super::tests::fixture()?;
    let id = setup(&mut app)?;
    for tab in [0, 2] {
        app.repository_view_mut()?.tab = tab;
        for width in [40, 80, 120] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 32))?;
            terminal.draw(|frame| draw(frame, &mut app))?;
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
            assert!(text.contains("Commits") && text.contains("Tags"));
            let commits_tab = text.find("Commits [c]").context("Commits tab")?;
            let info_tab = text.find("Info [i]").context("Info tab")?;
            let tags_tab = text.find("Tags [t]").context("Tags tab")?;
            assert!(commits_tab < info_tab && info_tab < tags_tab);
            assert!(text.contains(if tab == 0 { "Newest change" } else { "2.6.1" }));
            assert!(!text.contains("not loaded"));
        }
    }
    app.receive_repository(id, Reply::Changes(json!({"detailLabel":"Commit new", "detailChanges":{"head":"new","files":[{"item":{"path":"/first"},"diff":"-old\n+new"}]}})));
    for (width, height) in [(40, 24), (120, 32), (15, 6)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
        terminal.draw(|frame| draw(frame, &mut app))?;
        if width > 15 {
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
            assert!(text.contains("Commit new") && text.contains("+new"));
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|c| c.symbol() == "+" && c.fg == Color::Green)
            );
        }
    }
    Ok(())
}
