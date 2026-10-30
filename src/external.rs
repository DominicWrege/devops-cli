use crate::{
    config::{Store, Target},
    service::{response_id, segment},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn repo_url(target: &Target, repo: &Value) -> Result<String> {
    let name = repo["name"]
        .as_str()
        .filter(|v| !v.is_empty())
        .or_else(|| repo["id"].as_str().filter(|v| !v.is_empty()))
        .context("Repository has no name or ID")?;
    Ok(format!(
        "https://dev.azure.com/{}/{}/_git/{}",
        segment(&target.organization),
        segment(&target.project),
        segment(name)
    ))
}

pub fn pr_url(target: &Target, pr: &Value) -> Result<String> {
    Ok(format!(
        "{}/pullrequest/{}",
        repo_url(target, &pr["repository"])?,
        response_id(pr, "pullRequestId")?
    ))
}

pub fn browse(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)?;
    ensure!(
        parsed.scheme() == "https" && parsed.host_str() == Some("dev.azure.com"),
        "Invalid Azure DevOps web URL"
    );
    open::that_detached(url).context("Could not open the default browser")
}

#[derive(Default, Deserialize, Serialize)]
pub struct EditorSettings {
    pub executable: String,
    pub checkouts: BTreeMap<String, PathBuf>,
}

impl EditorSettings {
    pub fn load(store: &Store) -> Result<Self> {
        Ok(store.read("editor.json")?.unwrap_or_default())
    }
    pub fn executable(&self) -> String {
        std::env::var("DEVOPS_EDITOR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| {
                if self.executable.is_empty() {
                    "code".into()
                } else {
                    self.executable.clone()
                }
            })
    }
}

fn command(executable: &str) -> Command {
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: keep helper consoles hidden.
    }
    command
}

pub fn checkout(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .context("Local checkout directory does not exist")?;
    ensure!(path.is_dir(), "Local checkout must be a directory");
    let output = command("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "--show-toplevel"])
        .stdout(Stdio::piped())
        .output()
        .context("Git is required to locate the local checkout")?;
    ensure!(output.status.success(), "Choose an existing Git checkout");
    let root = String::from_utf8(output.stdout).context("Checkout path is not UTF-8")?;
    // Git returns an absolute path in a form editors can use, without Windows'
    // verbatim-path prefix introduced by canonicalize(). Preserve spaces in names.
    let root = PathBuf::from(root.trim_end_matches(['\r', '\n']));
    ensure!(
        root.is_absolute() && root.is_dir(),
        "Git returned an invalid checkout directory"
    );
    Ok(root)
}

pub fn open_editor(
    store: &Store,
    repo_key: &str,
    path: &Path,
    executable: &str,
) -> Result<PathBuf> {
    ensure!(
        !executable.trim().is_empty(),
        "Enter an editor executable, such as code or its full path"
    );
    let root = checkout(path)?;
    let mut settings = EditorSettings::load(store)?;
    let mut child = command(executable).arg(&root).spawn().context(
        "Could not launch editor. Enter a GUI editor executable or full path, without arguments",
    )?;
    // Reap editor launchers without blocking the TUI or killing the editor on exit.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    settings.executable = executable.to_owned();
    settings.checkouts.insert(repo_key.to_owned(), root.clone());
    store.write("editor.json", &settings)?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn web_links_encode_names_and_never_use_api_or_untrusted_urls() -> Result<()> {
        let target = Target {
            organization: "org".into(),
            project: "A Project".into(),
        };
        let repo =
            json!({"name":"app & tools","url":"https://evil.invalid","webUrl":"file:///secret"});
        let url = repo_url(&target, &repo)?;
        assert_eq!(
            url,
            "https://dev.azure.com/org/A%20Project/_git/app%20%26%20tools"
        );
        assert_eq!(
            pr_url(&target, &json!({"repository":repo,"pullRequestId":42}))?,
            format!("{url}/pullrequest/42")
        );
        assert!(repo_url(&target, &json!({})).is_err());
        assert!(
            pr_url(
                &target,
                &json!({"repository":{"name":"repo"},"pullRequestId":0})
            )
            .is_err()
        );
        assert!(browse("file:///secret").is_err());
        Ok(())
    }

    #[test]
    fn checkout_resolves_subdirectories_without_modifying_them() -> Result<()> {
        let root = tempfile::tempdir()?;
        let repo = root.path().join("checkout with spaces");
        std::fs::create_dir(&repo)?;
        assert!(checkout(&repo).is_err());
        assert!(command("git").arg("init").arg(&repo).status()?.success());
        let child = repo.join("src");
        std::fs::create_dir(&child)?;
        let file = child.join("uncommitted.txt");
        std::fs::write(&file, "keep this")?;
        assert_eq!(checkout(&child)?.canonicalize()?, repo.canonicalize()?);
        assert_eq!(std::fs::read_to_string(&file)?, "keep this");
        let store = Store {
            root: root.path().join("settings"),
        };
        assert!(open_editor(&store, "repo", &child, "devops-missing-test-editor-9271").is_err());
        assert!(!store.root.join("editor.json").exists());
        Ok(())
    }

    #[test]
    fn editor_mappings_are_separate_for_each_repository() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store {
            root: root.path().to_path_buf(),
        };
        let settings = EditorSettings {
            executable: "my editor.exe".into(),
            checkouts: BTreeMap::from([
                ("repo-one".into(), PathBuf::from("one")),
                ("repo-two".into(), PathBuf::from("two")),
            ]),
        };
        store.write("editor.json", &settings)?;
        let saved = EditorSettings::load(&store)?;
        assert_eq!(saved.executable, "my editor.exe");
        assert_eq!(saved.checkouts["repo-one"], PathBuf::from("one"));
        assert_eq!(saved.checkouts["repo-two"], PathBuf::from("two"));
        Ok(())
    }
}
