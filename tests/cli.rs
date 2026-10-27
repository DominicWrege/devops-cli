#![deny(unsafe_code, clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;

fn command(home: &std::path::Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("devops"));
    command.env("HOME", home).env("USERPROFILE", home);
    command.timeout(std::time::Duration::from_secs(5));
    command
}

#[test]
fn help_is_text_only_and_has_expected_commands() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("login"))
        .stdout(predicate::str::contains("pipeline"))
        .stdout(predicate::str::contains("tui"))
        .stdout(predicate::str::contains("--json").not());
    Ok(())
}
#[test]
fn missing_acknowledgement_fails_before_authentication() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    for args in [
        vec!["pr", "merge", "7", "--strategy", "squash"],
        vec!["pr", "abandon", "7"],
        vec!["pipeline", "cancel", "9"],
    ] {
        command(home.path())
            .args(args)
            .assert()
            .code(2)
            .stdout("")
            .stderr(predicate::str::contains("--yes"));
    }
    Ok(())
}
#[test]
fn init_persists_defaults_and_overrides_do_not_save() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .args([
            "init",
            "--org",
            "https://dev.azure.com/saved",
            "--project",
            "default",
            "--tenant-id",
            "11111111-1111-1111-1111-111111111111",
            "--client-id",
            "22222222-2222-2222-2222-222222222222",
        ])
        .assert()
        .success();
    let path = home.path().join(".config/devops/settings.json");
    let before = std::fs::read(&path)?;
    command(home.path())
        .args(["config", "--org", "override", "--project", "other"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Organization: saved"));
    assert_eq!(std::fs::read(&path)?, before);
    assert!(!String::from_utf8(before)?.contains("token"));
    Ok(())
}
#[test]
fn tui_refuses_redirected_input_without_altering_terminal() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .arg("tui")
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("interactive terminal"));
    Ok(())
}
#[test]
fn no_json_output_option_or_removed_auth_command() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .args(["auth", "status"])
        .assert()
        .code(2);
    command(home.path())
        .args(["pr", "list", "--json"])
        .assert()
        .code(2);
    Ok(())
}

#[test]
fn login_without_configuration_fails_before_network_or_prompting() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .arg("login")
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("--org"));
    Ok(())
}

#[test]
fn invalid_repository_names_fail_before_authentication() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    for name in ["", ".", "..", "a/b", "a\\b", "bad\nname"] {
        command(home.path())
            .args(["repo", "create", name])
            .assert()
            .code(2)
            .stdout("")
            .stderr(predicate::str::contains("Repository"));
    }
    Ok(())
}
