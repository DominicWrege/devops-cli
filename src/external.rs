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

fn command(executable: impl AsRef<std::ffi::OsStr>) -> Command {
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

fn spawn_editor(executable: &str, root: &Path) -> std::io::Result<std::process::Child> {
    let result = command(executable).arg(root).spawn();
    #[cfg(windows)]
    if Path::new(executable).extension().is_none()
        && matches!(&result, Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        // Windows editor commands can be batch launchers. Rust only adds .exe
        // when searching PATH, so try their extensions explicitly as well.
        for extension in ["cmd", "bat"] {
            let launcher = Path::new(executable).with_extension(extension);
            // Rust can start cmd.exe for a full batch path even when that
            // script is missing. Keep searching rather than saving a bad path.
            if launcher.components().count() > 1 && !launcher.is_file() {
                continue;
            }
            let result = command(launcher).arg(root).spawn();
            if !matches!(&result, Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
                return result;
            }
        }
    }
    result
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

fn checkout_or_clone(repo_url: &str, path: &Path) -> Result<PathBuf> {
    if !path
        .try_exists()
        .context("Could not check the local checkout directory")?
    {
        let output = command("git")
            .args(["clone", "--"])
            .arg(repo_url)
            .arg(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stderr(Stdio::piped())
            .output()
            .context("Git is required to clone the repository")?;
        ensure!(
            output.status.success(),
            "Could not clone repository: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    checkout(path)
}

pub fn open_editor(
    store: &Store,
    repo_key: &str,
    path: &Path,
    executable: &str,
) -> Result<PathBuf> {
    ensure!(
        !executable.trim().is_empty(),
        "Enter an editor executable name or full path"
    );
    let mut settings = EditorSettings::load(store)?;
    let root = checkout_or_clone(repo_key, path)?;
    let mut child = spawn_editor(executable, &root)
        .context("Could not launch editor. Check the executable name or full path")?;
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
    fn missing_checkout_clones_repository_and_preserves_existing_work() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source repo");
        assert!(
            command("git")
                .args(["init", "--initial-branch=main"])
                .arg(&source)
                .status()?
                .success()
        );
        std::fs::write(source.join("tracked.txt"), "cloned content")?;
        assert!(
            command("git")
                .arg("-C")
                .arg(&source)
                .args(["add", "tracked.txt"])
                .status()?
                .success()
        );
        assert!(
            command("git")
                .arg("-C")
                .arg(&source)
                .args([
                    "-c",
                    "user.name=Devops Test",
                    "-c",
                    "user.email=devops-test@example.invalid",
                    "commit",
                    "--no-gpg-sign",
                    "-m",
                    "Initial commit",
                ])
                .status()?
                .success()
        );
        let destination = root.path().join("new parent").join("checkout & tools");
        let checkout = checkout_or_clone(source.to_str().context("Source path")?, &destination)?;
        assert_eq!(checkout.canonicalize()?, destination.canonicalize()?);
        assert_eq!(
            std::fs::read_to_string(checkout.join("tracked.txt"))?,
            "cloned content"
        );
        let branch = command("git")
            .arg("-C")
            .arg(&checkout)
            .args(["branch", "--show-current"])
            .stdout(Stdio::piped())
            .output()?;
        assert!(branch.status.success());
        assert_eq!(String::from_utf8(branch.stdout)?.trim(), "main");
        std::fs::write(checkout.join("tracked.txt"), "uncommitted work")?;
        let subdir = checkout.join("src");
        std::fs::create_dir(&subdir)?;
        // An existing checkout needs no remote access or clone, including from a subdirectory.
        assert_eq!(checkout_or_clone("missing-remote", &subdir)?, checkout);
        assert_eq!(
            std::fs::read_to_string(checkout.join("tracked.txt"))?,
            "uncommitted work"
        );
        Ok(())
    }

    #[test]
    fn clone_failures_and_existing_non_checkouts_do_not_save_editor_settings() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store {
            root: root.path().join("settings"),
        };
        let missing_source = root.path().join("missing source");
        let destination = root.path().join("missing checkout");
        let error = open_editor(
            &store,
            missing_source.to_str().context("Source path")?,
            &destination,
            "devops-missing-test-editor-9271",
        )
        .err()
        .context("Expected clone failure")?;
        assert!(error.to_string().contains("Could not clone repository"));
        assert!(!destination.exists());
        assert!(!store.root.join("editor.json").exists());
        std::fs::create_dir(&destination)?;
        let keep = destination.join("keep.txt");
        std::fs::write(&keep, "keep this")?;
        let error = checkout_or_clone("missing-remote", &destination)
            .err()
            .context("Expected invalid checkout")?;
        assert!(
            error
                .to_string()
                .contains("Choose an existing Git checkout")
        );
        assert_eq!(std::fs::read_to_string(keep)?, "keep this");
        assert!(!destination.join(".git").exists());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_editor_launchers_support_names_and_paths() -> Result<()> {
        const LAUNCHER: &str = "DEVOPS_TEST_EDITOR_LAUNCHER";
        if let Some(launcher) = std::env::var_os(LAUNCHER) {
            let launcher = PathBuf::from(launcher);
            let root = launcher
                .parent()
                .context("Launcher directory")?
                .join("checkout & tools");
            for executable in [
                PathBuf::from("devops-fixture-editor-9271"),
                launcher.with_extension(""),
                launcher,
            ] {
                let mut child = spawn_editor(executable.to_str().context("Editor path")?, &root)?;
                let status = child.wait()?;
                ensure!(
                    status.success(),
                    "Editor launcher {executable:?} failed: {status}"
                );
                let marker = root.join("opened.txt");
                assert_eq!(std::fs::read_to_string(&marker)?.trim(), "opened");
                std::fs::remove_file(marker)?;
            }
            return Ok(());
        }

        let root = tempfile::tempdir()?;
        for extension in ["cmd", "bat"] {
            let bin = root.path().join(format!("{extension} editor bin"));
            std::fs::create_dir_all(bin.join("checkout & tools"))?;
            let launcher = bin.join(format!("devops-fixture-editor-9271.{extension}"));
            std::fs::write(
                &launcher,
                "@echo off\r\nif not \"%~2\"==\"\" exit /b 2\r\n> \"%~1\\opened.txt\" echo opened\r\n",
            )?;
            let mut paths = vec![bin];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            // Give only this test subprocess a fixture PATH; other tests keep
            // their environment, and no real editor is opened.
            let output = command(std::env::current_exe()?)
                .args([
                    "--exact",
                    "external::tests::windows_editor_launchers_support_names_and_paths",
                ])
                .env(LAUNCHER, launcher)
                .env("PATH", std::env::join_paths(paths)?)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()?;
            ensure!(
                output.status.success(),
                "Editor launcher test failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
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
