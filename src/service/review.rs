use super::*;

const MAX_DIFF_BYTES: usize = 1024 * 1024;

impl Service {
    /// Freeze the latest iteration and its merge base so subsequent pushes cannot mix revisions.
    pub async fn pr_changes(&self, pr: &Value) -> Result<Value> {
        let id = response_id(pr, "pullRequestId")?;
        let repo = Self::repo_id(pr)?;
        let iterations = rows(&call!(self.git().pull_request_iterations_client().list(
            self.org(),
            &repo,
            id,
            self.project()
        ))?);
        let latest = iterations
            .iter()
            .max_by_key(|v| v["id"].as_i64().unwrap_or(0))
            .context("No review iterations available. Open this PR in the browser with b.")?;
        let iteration = response_id(latest, "id")?;
        let base = text(&latest["commonRefCommit"], "commitId");
        let head = text(&latest["sourceRefCommit"], "commitId");
        ensure!(
            !base.is_empty() && !head.is_empty(),
            "Review iteration has no merge-base/source commit. Open this PR in the browser with b."
        );
        let mut files = Vec::new();
        let mut skip = 0;
        loop {
            let page = call!(
                self.git()
                    .pull_request_iteration_changes_client()
                    .get(self.org(), &repo, id, iteration, self.project())
                    .compare_to(0)
                    .top(100)
                    .skip(skip)
            )?;
            let entries = page["changeEntries"]
                .as_array()
                .context("Changes response has no file list")?;
            files.extend(
                entries
                    .iter()
                    .filter(|v| v["item"]["isFolder"] != true)
                    .cloned(),
            );
            let next = page["nextSkip"].as_i64().unwrap_or(0);
            if next == 0 {
                break;
            }
            ensure!(next > i64::from(skip), "Changes pagination did not advance");
            skip = i32::try_from(next).context("Changes pagination is out of range")?;
        }
        Ok(
            json!({"repository":repo, "iteration":iteration, "base":base, "head":head, "files":files}),
        )
    }

    async fn review_item(&self, repo: &str, path: &str, commit: &str) -> Result<Value> {
        call!(
            self.git()
                .items_client()
                .get(self.org(), repo, path, self.project())
                .version_descriptor_version(commit)
                .version_descriptor_version_type(git::GitVersionType::Commit)
                .format("json")
                .include_content_metadata(true)
                .include_content(true)
        )
    }

    pub async fn pr_file_diff(&self, changes: &Value, index: usize) -> Result<String> {
        let file = changes["files"]
            .get(index)
            .context("No changed file selected")?;
        let path = text(&file["item"], "path");
        ensure!(!path.is_empty(), "Changed file has no path");
        let kind = text(file, "changeType").to_ascii_lowercase();
        let flags: Vec<_> = kind.split(',').map(str::trim).collect();
        let added = flags.contains(&"add");
        let deleted = flags.contains(&"delete");
        let old_path = file["originalPath"]
            .as_str()
            .filter(|p| !p.is_empty())
            .unwrap_or(path);
        if file["item"]["gitObjectType"] == "commit" {
            return Ok("Submodule change. Open the PR in the browser with b to inspect it.".into());
        }
        let repo = text(changes, "repository");
        let base = text(changes, "base");
        let head = text(changes, "head");
        ensure!(
            !repo.is_empty() && !base.is_empty() && !head.is_empty(),
            "Incomplete review snapshot"
        );
        let (old, new) = tokio::try_join!(
            async {
                if added {
                    Ok(json!({"content":""}))
                } else {
                    self.review_item(repo, old_path, base).await
                }
            },
            async {
                if deleted {
                    Ok(json!({"content":""}))
                } else {
                    self.review_item(repo, path, head).await
                }
            },
        )?;
        if old["contentMetadata"]["isBinary"] == true || new["contentMetadata"]["isBinary"] == true
        {
            return Ok("Binary file. Open the PR in the browser with b to inspect it.".into());
        }
        let old_text = old["content"]
            .as_str()
            .context("Base file content unavailable; open the PR in the browser with b")?;
        let new_text = new["content"]
            .as_str()
            .context("Source file content unavailable; open the PR in the browser with b")?;
        if old_text.contains('\0') || new_text.contains('\0') {
            return Ok("Binary file. Open the PR in the browser with b to inspect it.".into());
        }
        if old_text.len() > MAX_DIFF_BYTES || new_text.len() > MAX_DIFF_BYTES {
            return Ok(
                "File exceeds the 1 MiB text diff limit. Open the PR in the browser with b.".into(),
            );
        }
        let before = if added {
            "/dev/null".into()
        } else {
            format!("a/{}", old_path.trim_start_matches('/'))
        };
        let after = if deleted {
            "/dev/null".into()
        } else {
            format!("b/{}", path.trim_start_matches('/'))
        };
        let diff = similar::TextDiff::configure()
            .timeout(std::time::Duration::from_millis(500))
            .diff_lines(old_text, new_text)
            .unified_diff()
            .context_radius(3)
            .header(&before, &after)
            .to_string();
        Ok(if diff.is_empty() {
            format!(
                "{kind}: {old_path} -> {path}\nNo text changes (rename, metadata, or empty file)."
            )
        } else {
            output::clean(&diff)
        })
    }
}
