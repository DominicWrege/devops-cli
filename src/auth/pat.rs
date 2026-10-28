use crate::{
    config::{Store, organization},
    error,
};
use anyhow::Result;
#[cfg(windows)]
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct SavedPat {
    value: String,
}

pub fn validate(token: &str) -> Result<()> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(error::invalid(
            "Personal access token must be non-empty and contain no whitespace or non-ASCII characters (check DEVOPS_PAT if set).",
        ));
    }
    Ok(())
}

pub fn environment() -> Result<Option<String>> {
    match std::env::var("DEVOPS_PAT") {
        Ok(token) => {
            validate(&token)?;
            Ok(Some(token))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(error::invalid(
            "DEVOPS_PAT must contain a valid personal access token",
        )),
    }
}

pub fn required() -> anyhow::Error {
    error::Failure {
        code: 3,
        message: "Personal access token required. Run `devops login` in an interactive terminal to enter it, or set DEVOPS_PAT.".into(),
    }.into()
}

pub fn load(store: &Store, org: &str) -> Result<Option<String>> {
    let name = format!("pat-{}.json", organization(org)?);
    #[cfg(windows)]
    let saved: Option<SavedPat> = store.read(&name)?;
    #[cfg(not(windows))]
    let saved: Option<SavedPat> = store.read_private(&name)?;
    let Some(saved) = saved else { return Ok(None) };
    #[cfg(windows)]
    let token = {
        let data = STANDARD
            .decode(&saved.value)
            .map_err(|_| anyhow::anyhow!("Invalid saved PAT. Run `devops login` to replace it."))?;
        String::from_utf8(super::protect::unprotect(&data)?)
            .map_err(|_| anyhow::anyhow!("Invalid saved PAT. Run `devops login` to replace it."))?
    };
    #[cfg(not(windows))]
    let token = saved.value;
    validate(&token)?;
    Ok(Some(token))
}

pub fn save(store: &Store, org: &str, token: &str) -> Result<()> {
    validate(token)?;
    let name = format!("pat-{}.json", organization(org)?);
    #[cfg(windows)]
    let value = STANDARD.encode(super::protect::protect(token.as_bytes())?);
    #[cfg(not(windows))]
    let value = token.to_owned();
    store.write(&name, &SavedPat { value })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_pats_round_trip_and_are_scoped_to_the_organization() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store {
            root: dir.path().join("config"),
        };
        assert!(load(&store, "org")?.is_none());
        save(&store, "org", "fixture-secret-pat")?;
        assert_eq!(load(&store, "org")?.as_deref(), Some("fixture-secret-pat"));
        assert!(load(&store, "other-org")?.is_none());
        assert!(!store.root.join("settings.json").exists());
        #[cfg(windows)]
        assert!(
            !std::fs::read_to_string(store.root.join("pat-org.json"))?
                .contains("fixture-secret-pat")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(store.root.join("pat-org.json"))?
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        Ok(())
    }

    #[test]
    fn replacing_a_pat_is_atomic_and_invalid_input_keeps_the_previous_token() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store {
            root: dir.path().join("config"),
        };
        save(&store, "org", "old-pat")?;
        save(&store, "org", "new-pat")?;
        assert_eq!(load(&store, "org")?.as_deref(), Some("new-pat"));
        assert!(save(&store, "org", "invalid secret").is_err());
        assert_eq!(load(&store, "org")?.as_deref(), Some("new-pat"));
        assert!(save(&store, "../escape", "fixture-secret").is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_pat_cache_rejects_unsafe_permissions_and_symlinks() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir()?;
        let store = Store {
            root: dir.path().join("config"),
        };
        save(&store, "org", "fixture-secret")?;
        let path = store.root.join("pat-org.json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        assert!(load(&store, "org").is_err());
        std::fs::remove_file(&path)?;
        symlink(dir.path().join("elsewhere"), &path)?;
        assert!(load(&store, "org").is_err());
        assert!(save(&store, "org", "fixture-secret").is_err());
        Ok(())
    }
}
