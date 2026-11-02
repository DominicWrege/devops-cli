use super::*;
use std::collections::{BTreeSet, HashMap};

mod tags;
pub(crate) use tags::{TagChange, tag_name};

pub(crate) fn ref_commit(value: &Value) -> &str {
    value["peeledObjectId"]
        .as_str()
        .filter(|id| !id.is_empty() && id.chars().any(|c| c != '0'))
        .unwrap_or_else(|| text(value, "objectId"))
}

#[cfg(test)]
mod tests;

pub(crate) fn commit_date(value: &Value) -> Option<time::OffsetDateTime> {
    let date = value["committer"]["date"]
        .as_str()
        .or_else(|| value["author"]["date"].as_str())?;
    time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339).ok()
}

impl Service {
    pub async fn repo_commits(
        &self,
        repo: &str,
        version: &str,
        pinned: bool,
        skip: usize,
    ) -> Result<Vec<Value>> {
        Ok(rows(&call!(
            self.git()
                .commits_client()
                .get_commits(self.org(), segment(repo), self.project())
                .search_criteria_item_version_version(version)
                .search_criteria_item_version_version_type(if pinned {
                    git::GitVersionType::Commit
                } else {
                    git::GitVersionType::Branch
                })
                .search_criteria_show_oldest_commits_first(false)
                .search_criteria_skip(i32::try_from(skip).context("Commit offset is too large")?)
                .search_criteria_top(100)
        )?))
    }

    pub async fn repo_refs(&self, repo: &str, tags: bool) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        let mut token = String::new();
        let mut seen = BTreeSet::new();
        loop {
            let mut request = self
                .git()
                .refs_client()
                .list(self.org(), segment(repo), self.project())
                .filter(if tags { "tags/" } else { "heads/" })
                .peel_tags(tags)
                .top(1000);
            if !token.is_empty() {
                request = request.continuation_token(&token);
            }
            let response = send(request.send()).await?.into_raw_response();
            let next = response
                .headers()
                .get_optional_str(&"x-ms-continuationtoken".into())
                .unwrap_or_default()
                .to_owned();
            values.extend(rows(&decode(response)?));
            if next.is_empty() {
                break;
            }
            ensure!(
                seen.insert(next.clone()),
                "Reference pagination did not advance"
            );
            token = next;
        }
        if tags {
            // Ref listings contain no dates. Resolve unique, peeled targets in batches
            // so chronological ordering also works for lightweight and shared tags.
            let ids: Vec<_> = values
                .iter()
                .map(ref_commit)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let mut commits = HashMap::new();
            for chunk in ids.chunks(100) {
                let body = git::GitQueryCommitsCriteria {
                    ids: chunk.to_vec(),
                    ..Default::default()
                };
                let page = call!(self.git().commits_client().get_commits_batch(
                    self.org(),
                    body,
                    segment(repo),
                    self.project()
                ))?;
                for commit in rows(&page) {
                    commits.insert(text(&commit, "commitId").to_owned(), commit);
                }
            }
            for value in &mut values {
                if let Some(commit) = commits.get(ref_commit(value)) {
                    value["targetCommit"] = commit.clone();
                }
            }
        }
        Ok(values)
    }

    pub async fn repo_commit_changes(&self, repo: &str, commit: &str) -> Result<Value> {
        ensure!(!commit.is_empty(), "No commit selected");
        let value = call!(self.git().commits_client().get(
            self.org(),
            commit,
            segment(repo),
            self.project()
        ))?;
        let base = value["parents"][0].as_str().unwrap_or_default();
        let mut files = Vec::new();
        let mut skip = 0;
        loop {
            let page = call!(
                self.git()
                    .commits_client()
                    .get_changes(self.org(), commit, segment(repo), self.project())
                    .top(100)
                    .skip(skip)
            )?;
            let entries = page["changes"]
                .as_array()
                .context("Commit response has no changes")?;
            files.extend(
                entries
                    .iter()
                    .filter(|v| v["item"]["isFolder"] != true)
                    .cloned(),
            );
            if entries.len() < 100 {
                break;
            }
            skip = skip
                .checked_add(100)
                .context("Change offset is too large")?;
        }
        Ok(json!({
            "detailLabel":format!("Commit {} · {}", commit.chars().take(7).collect::<String>(), text(&value, "comment").lines().next().unwrap_or_default()),
            "detailCommit":value,
            "detailChanges":{"repository":repo, "base":base, "head":commit, "files":files},
            "detailChangeIndex":0
        }))
    }
}
