use super::*;

pub(in crate::tui) struct TagForm {
    key: String,
    repo: String,
    pub original: Option<Value>,
}

pub(in crate::tui) struct TagWrite {
    key: String,
    pub repo: String,
    pub change: TagChange,
}

impl App {
    fn selected_tag(&self) -> Result<Value> {
        let view = self.repository_view().context("Open Tags first")?;
        ensure!(view.tab == 2 && !view.picking_branch, "Open Tags first");
        view.tags
            .as_ref()
            .and_then(|tags| tags.get(view.selected_ref()?))
            .cloned()
            .context("Select an individual tag first")
    }

    pub(super) fn open_tag_form(&mut self, rename: bool) -> Result<()> {
        let (key, repo) = self.repository()?;
        let original = if rename {
            Some(self.selected_tag()?)
        } else {
            None
        };
        let fields = if let Some(tag) = &original {
            vec![(
                "Name",
                text(tag, "name").trim_start_matches("refs/tags/").into(),
            )]
        } else {
            let view = self.repository_view().context("Open a repository first")?;
            // The Tags selection points at an existing release, which may be old.
            // New tags use the commit chosen in the current branch's history.
            let target = view
                .histories
                .get(&view.branch)
                .and_then(|history| {
                    history
                        .commits
                        .get(history.selection.selected().unwrap_or(0))?["commitId"]
                        .as_str()
                })
                .unwrap_or_default()
                .to_owned();
            vec![("Name", String::new()), ("Commit SHA", target)]
        };
        self.status.clear();
        self.form = Some(Form::new(
            FormKind::Tag(TagForm {
                key,
                repo: text(&repo, "id").into(),
                original,
            }),
            fields,
        ));
        Ok(())
    }

    pub(in crate::tui) fn submit_tag(&mut self) -> Result<()> {
        let form = self.form.as_ref().context("No form")?;
        let FormKind::Tag(edit) = &form.kind else {
            anyhow::bail!("Not a tag form")
        };
        let name = tag_name(form.value(0)?)?;
        let change = if let Some(tag) = &edit.original {
            TagChange::Rename {
                old: text(tag, "name").into(),
                name,
                object: text(tag, "objectId").into(),
            }
        } else {
            TagChange::Create {
                name,
                target: form.value(1)?.into(),
            }
        };
        change.validate()?;
        if let Some(tags) = self
            .repo_views
            .get(&edit.key)
            .and_then(|view| view.tags.as_ref())
        {
            ensure!(
                !tags.iter().any(|tag| text(tag, "name") == change.name()),
                "A tag with that name already exists"
            );
        }
        let write = TagWrite {
            key: edit.key.clone(),
            repo: edit.repo.clone(),
            change,
        };
        self.execute_tag(write)
    }

    pub(super) fn confirm_tag_delete(&mut self) -> Result<()> {
        let (key, repo) = self.repository()?;
        let tag = self.selected_tag()?;
        self.tag_confirm = Some(TagWrite {
            key,
            repo: text(&repo, "id").into(),
            change: TagChange::Delete {
                name: text(&tag, "name").into(),
                object: text(&tag, "objectId").into(),
            },
        });
        Ok(())
    }

    pub(in crate::tui) fn execute_tag(&mut self, write: TagWrite) -> Result<()> {
        write.change.validate()?;
        let api = self.api()?;
        self.launch(async move {
            api.repo_change_tag(&write.repo, &write.change).await?;
            let refreshed = api
                .repo_refs(&write.repo, true)
                .await
                .map_err(|error| output::clean(&error.to_string()));
            Ok(Message::Repository {
                key: write.key,
                reply: Reply::TagWritten {
                    change: write.change,
                    refreshed,
                },
            })
        });
        self.loading_label = "Saving tag";
        self.writing = true;
        Ok(())
    }

    pub(super) fn receive_tag_write(
        &mut self,
        key: String,
        change: TagChange,
        refreshed: Result<Vec<Value>, String>,
    ) {
        let Some(view) = self.repo_views.get_mut(&key) else {
            return;
        };
        let old_index = view.tags_selection.selected().unwrap_or(0);
        let mut values = view.tags.take().unwrap_or_default();
        match &change {
            TagChange::Create { name, target } => {
                values.push(serde_json::json!({"name":name,"objectId":target}));
            }
            TagChange::Rename { old, name, .. } => {
                if let Some(tag) = values.iter_mut().find(|tag| text(tag, "name") == old) {
                    tag["name"] = serde_json::json!(name);
                }
            }
            TagChange::Delete { name, .. } => values.retain(|tag| text(tag, "name") != name),
        }
        let refresh_error = match refreshed {
            Ok(fresh) => {
                values = fresh;
                None
            }
            Err(error) => Some(error),
        };
        if !matches!(change, TagChange::Delete { .. }) {
            // Reveal new/renamed tags even when their destination folder was folded.
            let short = change.name().trim_start_matches("refs/tags/");
            view.collapsed_tags
                .retain(|path| !short.starts_with(path.as_str()));
        }
        let rows = ref_rows(&values, false, &view.collapsed_tags);
        let index = rows
            .iter()
            .position(|row| row.path == change.name())
            .unwrap_or_else(|| old_index.min(rows.len().saturating_sub(1)));
        view.tags = Some(values);
        view.tags_selection
            .select(if rows.is_empty() { None } else { Some(index) });
        self.form = None;
        self.tag_confirm = None;
        self.status = change.message();
        if let Some(error) = refresh_error {
            self.status.push_str(&format!(
                ". Tag saved, but refresh failed: {error}. Press F5 to refresh."
            ));
        }
    }
}
