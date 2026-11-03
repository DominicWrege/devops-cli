use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Directory,
    File,
}

pub(super) struct Completion {
    pub paths: Vec<PathBuf>,
    pub selected: usize,
}

impl Completion {
    pub fn new(value: &str, kind: Kind) -> Result<Self> {
        let path = expand_path(value);
        let (directory, prefix) = if value.ends_with(std::path::is_separator) || value.is_empty() {
            (path.as_path(), String::new())
        } else {
            (
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase(),
            )
        };
        let mut paths = Vec::new();
        for entry in fs::read_dir(directory).context("Cannot read path suggestions")? {
            let entry = entry?;
            let path = entry.path();
            if (path.is_dir() || (matches!(kind, Kind::File) && path.is_file()))
                && entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .starts_with(&prefix)
                && path.to_str().is_some()
            {
                paths.push(path);
            }
        }
        paths.sort_by_cached_key(|path| (!path.is_dir(), path.to_string_lossy().to_lowercase()));
        Ok(Self { paths, selected: 0 })
    }

    pub fn value(&self) -> Option<String> {
        self.paths.get(self.selected).map(|path| {
            let mut value = path.display().to_string();
            if path.is_dir() && !value.ends_with(std::path::is_separator) {
                value.push(std::path::MAIN_SEPARATOR);
            }
            value
        })
    }
}

pub(super) fn expand_path(value: &str) -> PathBuf {
    if (value == "~" || value.starts_with("~/") || value.starts_with("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(value.get(2..).unwrap_or_default());
    }
    if value.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suggestions_filter_folders_and_preserve_spaces() -> Result<()> {
        let root = tempfile::tempdir()?;
        let repo = root.path().join("Demo project");
        fs::create_dir(&repo)?;
        fs::write(root.path().join("demo.txt"), "fixture")?;
        fs::create_dir(root.path().join("other"))?;
        let completion = Completion::new(
            &root.path().join("dem").display().to_string(),
            Kind::Directory,
        )?;
        assert_eq!(completion.paths, vec![repo.clone()]);
        assert_eq!(completion.value().map(PathBuf::from), Some(repo.join("")));
        let empty = Completion::new(
            &format!("{}{sep}", repo.display(), sep = std::path::MAIN_SEPARATOR),
            Kind::Directory,
        )?;
        assert!(empty.paths.is_empty());
        assert!(
            Completion::new(
                &root.path().join("missing/child").display().to_string(),
                Kind::Directory
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn editor_files_and_home_paths_are_supported() -> Result<()> {
        let root = tempfile::tempdir()?;
        let editor = root.path().join("my editor.exe");
        fs::write(&editor, "fixture")?;
        let completion =
            Completion::new(&root.path().join("my ed").display().to_string(), Kind::File)?;
        assert_eq!(completion.paths, vec![editor]);
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_path("~"), home);
            assert_eq!(expand_path("~/source"), home.join("source"));
        }
        Ok(())
    }
}
