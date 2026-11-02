use super::*;

const ZERO: &str = "0000000000000000000000000000000000000000";

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub(crate) enum TagChange {
    Create {
        name: String,
        target: String,
    },
    Rename {
        old: String,
        name: String,
        object: String,
    },
    Delete {
        name: String,
        object: String,
    },
}

pub(crate) fn tag_name(value: &str) -> Result<String> {
    let name = value.strip_prefix("refs/tags/").unwrap_or(value);
    ensure!(
        !name.is_empty()
            && !name.ends_with('.')
            && !name.contains("..")
            && !name.contains("@{")
            && !name
                .chars()
                .any(|c| c.is_control() || " ~^:?*[\\".contains(c))
            && name
                .split('/')
                .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock")),
        "Invalid tag name: use a Git tag name such as v1.0.0 or release/1.0.0"
    );
    Ok(format!("refs/tags/{name}"))
}

fn object_id(value: &str) -> Result<()> {
    ensure!(
        value.len() == 40 && value.bytes().all(|c| c.is_ascii_hexdigit()) && value != ZERO,
        "Tag target must be a full, nonzero 40-character commit/object SHA"
    );
    Ok(())
}

impl TagChange {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Create { name, target } => {
                tag_name(name)?;
                object_id(target)?;
            }
            Self::Rename { old, name, object } => {
                ensure!(
                    tag_name(old)? != tag_name(name)?,
                    "Choose a different tag name"
                );
                object_id(object)?;
            }
            Self::Delete { name, object } => {
                tag_name(name)?;
                object_id(object)?;
            }
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Create { name, .. } | Self::Rename { name, .. } | Self::Delete { name, .. } => {
                name
            }
        }
    }

    pub fn message(&self) -> String {
        let name = self.name().trim_start_matches("refs/tags/");
        match self {
            Self::Create { .. } => format!("Created tag {name}"),
            Self::Rename { old, .. } => format!(
                "Renamed tag {} to {name}",
                old.trim_start_matches("refs/tags/")
            ),
            Self::Delete { .. } => format!("Deleted tag {name}"),
        }
    }
}

impl Service {
    pub async fn tag_action(&self, command: &TagCommand) -> Result<String> {
        command.validate()?;
        let (repo, change) = match command {
            TagCommand::Create { name, repo, target } => (
                repo,
                TagChange::Create {
                    name: name.clone(),
                    target: target.clone(),
                },
            ),
            TagCommand::Delete { name, repo, .. } => {
                let name = tag_name(name)?;
                // Azure DevOps filters by prefix. Match the full name and retain the
                // unpeeled object ID so annotated tags and concurrent updates are safe.
                let refs = self
                    .repo_refs_filtered(repo, name.strip_prefix("refs/").context("Tag ref")?, false)
                    .await?;
                let tag = refs
                    .iter()
                    .find(|tag| text(tag, "name") == name)
                    .with_context(|| {
                        format!("Tag {} not found", name.trim_start_matches("refs/tags/"))
                    })?;
                (
                    repo,
                    TagChange::Delete {
                        name,
                        object: text(tag, "objectId").into(),
                    },
                )
            }
        };
        self.repo_change_tag(repo, &change).await?;
        Ok(change.message())
    }

    async fn update_tag_ref(&self, repo: &str, name: &str, old: &str, new: &str) -> Result<()> {
        let name = tag_name(name)?;
        let value = call!(self.git().refs_client().update_refs(
            self.org(),
            vec![git::GitRefUpdate {
                name: Some(name.clone()),
                old_object_id: Some(old.into()),
                new_object_id: Some(new.into()),
                ..Default::default()
            }],
            segment(repo),
            self.project()
        ))?;
        let results = rows(&value);
        let result = results.iter().find(|r| text(r, "name") == name).context(
            "Azure DevOps returned no result for the tag update; refresh tags before retrying",
        )?;
        ensure!(
            result["success"] == true,
            "Could not update tag {}: {} {}. Refresh tags before retrying.",
            name.trim_start_matches("refs/tags/"),
            text(result, "updateStatus"),
            text(result, "customMessage")
        );
        Ok(())
    }

    pub async fn repo_change_tag(&self, repo: &str, change: &TagChange) -> Result<()> {
        change.validate()?;
        match change {
            TagChange::Create { name, target } => {
                self.update_tag_ref(repo, name, ZERO, target).await
            }
            TagChange::Delete { name, object } => {
                self.update_tag_ref(repo, name, object, ZERO).await
            }
            TagChange::Rename { old, name, object } => {
                // Create first so a duplicate or denied destination never removes the source.
                // Preserve the unpeeled object ID, including annotated tag metadata.
                self.update_tag_ref(repo, name, ZERO, object).await?;
                self.update_tag_ref(repo, old, object, ZERO).await.map_err(|error| anyhow::anyhow!(
                    "Created tag {}, but could not delete {}: {error}. Both names may exist; refresh tags before retrying.",
                    name.trim_start_matches("refs/tags/"), old.trim_start_matches("refs/tags/")))
            }
        }
    }
}
