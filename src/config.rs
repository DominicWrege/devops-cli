use crate::error::invalid;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
#[cfg(not(windows))]
use std::fs::{File, OpenOptions};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub organization: Option<String>,
    pub project: Option<String>,
    pub project_id: Option<String>,
    pub tenant_id: Option<String>,
    pub client_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub organization: String,
    pub project: String,
}

impl Store {
    pub fn home() -> Result<Self> {
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir);
        #[cfg(not(windows))]
        let home = dirs::home_dir();
        Ok(Self {
            root: home
                .context("Cannot determine home directory")?
                .join(".config/devops"),
        })
    }
    pub fn load(&self) -> Result<Settings> {
        Ok(self.read("settings.json")?.unwrap_or_default())
    }
    pub fn read<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.root.join(name);
        reject_symlink(&path)?;
        match fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data).map(Some).map_err(|_| {
                anyhow::anyhow!(
                    "Invalid {}. Repair or remove it through setup.",
                    path.display()
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("Cannot read configuration file"),
        }
    }
    #[cfg(not(windows))]
    pub fn read_session<T: serde::de::DeserializeOwned>(&self) -> Result<Option<T>> {
        self.prepare()?;
        let path = self.root.join("auth.json");
        reject_symlink(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            match fs::metadata(&path) {
                Ok(meta) => ensure!(
                    meta.permissions().mode() & 0o777 == 0o600,
                    "Authentication cache must be owner-only. Run chmod 600 on {}.",
                    path.display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        self.read("auth.json")
    }
    pub fn save(&self, settings: &Settings) -> Result<()> {
        self.write("settings.json", settings)
    }
    pub fn prepare(&self) -> Result<()> {
        reject_symlink(&self.root)?;
        fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        self.prepare()?;
        let path = self.root.join(name);
        reject_symlink(&path)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(path)
            .map_err(|_| anyhow::anyhow!("Cannot replace cache/configuration file"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    pub fn lock_file(&self) -> Result<File> {
        self.prepare()?;
        let path = self.root.join("auth.lock");
        reject_symlink(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        Ok(options.open(path)?)
    }
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => ensure!(
            !meta.file_type().is_symlink(),
            "Refusing a symbolic link at {}",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

impl Settings {
    pub fn target(&self, org: Option<&str>, project: Option<&str>) -> Result<Target> {
        let organization = organization(
            org.or(self.organization.as_deref())
                .ok_or_else(|| invalid("Missing organization. Use --org or TUI setup."))?,
        )?;
        let project = project
            .or(self.project.as_deref())
            .ok_or_else(|| invalid("Missing project. Use --project or TUI setup."))?
            .trim()
            .to_string();
        if project.is_empty()
            || matches!(project.as_str(), "." | "..")
            || project.contains(['/', '\\'])
            || project.chars().any(char::is_control)
        {
            return Err(invalid(
                "Project must be a project name or UUID, not a path",
            ));
        }
        Ok(Target {
            organization,
            project,
        })
    }
    pub fn identity(&self) -> Result<(String, String)> {
        let tenant = self.tenant_id.clone().ok_or_else(|| {
            invalid("Missing tenant ID. Configure it using `devops tui` or `devops init`.")
        })?;
        let client = self.client_id.clone().ok_or_else(|| invalid("Missing public-client application ID. Configure your own Entra registration through setup."))?;
        let tenant =
            uuid::Uuid::parse_str(&tenant).map_err(|_| invalid("Tenant ID must be a UUID"))?;
        let client =
            uuid::Uuid::parse_str(&client).map_err(|_| invalid("Client ID must be a UUID"))?;
        Ok((tenant.to_string(), client.to_string()))
    }
}

pub fn organization(input: &str) -> Result<String> {
    let name = input
        .trim()
        .strip_prefix("https://dev.azure.com/")
        .unwrap_or(input.trim())
        .trim_end_matches('/');
    if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err(invalid(
            "Organization must be a name or https://dev.azure.com/NAME URL",
        ));
    }
    Ok(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_never_change_defaults() -> Result<()> {
        let settings = Settings {
            organization: Some("saved".into()),
            project: Some("old".into()),
            ..Default::default()
        };
        let before = settings.clone();
        assert_eq!(
            settings.target(Some("https://dev.azure.com/new/"), Some("other"))?,
            Target {
                organization: "new".into(),
                project: "other".into()
            }
        );
        assert_eq!(settings, before);
        assert_eq!(settings.target(None, None)?.project, "old");
        Ok(())
    }
    #[test]
    fn reject_organization_path_injection() {
        for value in [
            "../bad",
            "org?x=y",
            "https://evil.example/org",
            "org/project",
            "",
        ] {
            assert!(organization(value).is_err());
        }
    }
    #[test]
    fn reject_project_paths_without_rejecting_spaces() -> Result<()> {
        let settings = Settings {
            organization: Some("org".into()),
            ..Default::default()
        };
        for project in [".", "..", "../other", "a/b", "a\\b", "", "\x1b"] {
            assert_eq!(
                settings
                    .target(None, Some(project))
                    .err()
                    .context("Expected invalid project")?
                    .downcast_ref::<crate::error::Failure>()
                    .context("Expected validation failure")?
                    .code,
                2
            );
        }
        assert_eq!(
            settings.target(None, Some("My Project"))?.project,
            "My Project"
        );
        Ok(())
    }
    #[test]
    fn roundtrip_and_permissions() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store {
            root: root.path().join("config"),
        };
        assert_eq!(store.load()?, Settings::default());
        store.save(&Settings::default())?;
        assert_eq!(store.load()?, Settings::default());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&store.root)?.permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(store.root.join("settings.json"))?
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn token_cache_rejects_symlinks_and_world_readable_files() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir()?;
        let store = Store {
            root: root.path().join("config"),
        };
        store.write("auth.json", &serde_json::json!({"secret":"fixture"}))?;
        let path = store.root.join("auth.json");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        assert!(
            store
                .read_session::<serde_json::Value>()
                .err()
                .context("Expected insecure cache error")?
                .to_string()
                .contains("owner-only")
        );
        fs::remove_file(&path)?;
        symlink(root.path().join("elsewhere"), &path)?;
        assert!(store.read_session::<serde_json::Value>().is_err());
        assert!(store.write("auth.json", &"secret").is_err());
        assert!(!root.path().join("elsewhere").exists());
        Ok(())
    }
}
