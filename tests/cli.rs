#![deny(unsafe_code, clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;

fn command(home: &std::path::Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("devops"));
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("DEVOPS_PAT");
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
        .stdout(predicate::str::contains("--json"));
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
fn json_output_and_removed_auth_command() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .args(["auth", "status"])
        .assert()
        .code(2);
    command(home.path())
        .args(["config", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"auth\":\"pat\""));
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

#[test]
fn pat_defaults_need_no_app_registration_and_never_store_the_token() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let secret = "fixture-personal-access-token";
    command(home.path())
        .env("DEVOPS_PAT", secret)
        .args(["init", "--pat", "--org", "org", "--project", "project"])
        .assert()
        .success()
        .stdout(predicate::str::contains(secret).not())
        .stderr(predicate::str::contains(secret).not());
    let saved = std::fs::read_to_string(home.path().join(".config/devops/settings.json"))?;
    let settings: serde_json::Value = serde_json::from_str(&saved)?;
    assert_eq!(settings["auth"], "pat");
    assert!(settings["tenant_id"].is_null());
    assert!(settings["client_id"].is_null());
    assert!(!saved.contains(secret));
    command(home.path())
        .env("DEVOPS_PAT", secret)
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("Authentication: PAT"))
        .stdout(predicate::str::contains(secret).not());
    // PAT mode stays selected without the secret; it must not fall back to WAM/device login.
    command(home.path())
        .args(["pr", "list"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(predicate::str::contains("DEVOPS_PAT"));
    Ok(())
}

#[test]
fn pat_login_rejects_missing_or_invalid_tokens_before_network_or_prompts() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .args(["login", "--pat", "--org", "org"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(predicate::str::contains("DEVOPS_PAT"));
    for secret in ["", "fixture secret", "fixture\nsecret", "é"] {
        let assertion = command(home.path())
            .env("DEVOPS_PAT", secret)
            .args(["login", "--pat", "--org", "org"])
            .assert()
            .code(2)
            .stdout("")
            .stderr(predicate::str::contains("DEVOPS_PAT"));
        if !secret.is_empty() {
            assertion.stderr(predicate::str::contains(secret).not());
        }
    }
    assert!(!home.path().join(".config/devops/settings.json").exists());
    Ok(())
}

#[test]
fn pat_environment_selects_pat_mode_without_a_flag() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .env("DEVOPS_PAT", "fixture-pat")
        .args(["init", "--org", "org", "--project", "project"])
        .assert()
        .success();
    command(home.path())
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("Authentication: PAT"));
    Ok(())
}

#[test]
fn pat_defaults_can_be_saved_without_a_token_and_switched_back_to_entra() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .args(["init", "--pat", "--org", "org", "--project", "project"])
        .assert()
        .success();
    command(home.path())
        .args([
            "init",
            "--tenant-id",
            "11111111-1111-1111-1111-111111111111",
            "--client-id",
            "22222222-2222-2222-2222-222222222222",
        ])
        .assert()
        .success();
    command(home.path())
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("Authentication: Entra"));
    Ok(())
}

#[test]
fn default_login_requests_a_pat_not_an_app_registration() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    command(home.path())
        .args(["login", "--org", "org"])
        .write_stdin("fixture-secret-pat\n")
        .assert()
        .code(3)
        .stdout("")
        .stderr(predicate::str::contains("Personal access token required"))
        .stderr(predicate::str::contains("app").not())
        .stderr(predicate::str::contains("fixture-secret-pat").not());
    // An explicit --pat flag is no longer required for new settings.
    command(home.path())
        .args(["init", "--org", "org", "--project", "project"])
        .assert()
        .success();
    command(home.path())
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("Authentication: PAT"));
    Ok(())
}

#[test]
fn pat_flag_conflicts_with_entra_parameters() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    for cmd in ["login", "init"] {
        for flag in ["--client-id", "--tenant-id"] {
            command(home.path())
                .args([cmd, "--pat", flag, "11111111-1111-1111-1111-111111111111"])
                .assert()
                .code(2)
                .stderr(predicate::str::contains("cannot be used with"));
        }
    }
    Ok(())
}

#[test]
fn json_config_is_parseable_and_tui_rejects_json() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    for args in [["--json", "config"], ["config", "--json"]] {
        let result = command(home.path()).args(args).assert().success();
        let value: serde_json::Value = serde_json::from_slice(&result.get_output().stdout)?;
        assert_eq!(value["auth"], "pat");
        assert!(value.get("message").is_none());
    }
    command(home.path())
        .args(["tui", "--json"])
        .assert()
        .code(2)
        .stdout("");
    command(home.path())
        .args(["pr", "abandon", "7", "--json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--yes"));
    Ok(())
}
