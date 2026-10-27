# DevOps

Azure DevOps in your terminal. Review pull requests, watch pipeline logs, and run commands from scripts or agents.

For Azure DevOps Services, not Azure infrastructure or self-hosted Azure DevOps Server. Supports YAML pipelines, pull requests, and empty remote repository creation. No work items or classic pipelines.

## Install

Install a published release with [mise](https://mise.jdx.dev/):

```sh
mise use -g github:DominicWrege/devops-cli@latest
devops tui
```

Use `@0.2.0` instead of `@latest` to pin the version. You can also download an archive from [Releases](https://github.com/DominicWrege/devops-cli/releases).

- Windows x64.
- Linux/WSL x64 with glibc 2.35 or newer.
- Linux/WSL ARM64 with glibc 2.39 or newer.

Install separately on Windows and inside WSL. No macOS or Alpine/musl release is available. Azure CLI is not required.

## Sign in

Run `devops tui`. The first launch opens setup; press `s` to reopen it later.

1. Enter your Entra tenant ID and your own public-client app ID.
2. Complete sign-in. Windows uses your Windows account through WAM. Linux/WSL shows a device code and a URL to open.
3. Choose an organization and default project.

Ask your tenant administrator for an app registration with Azure DevOps delegated `user_impersonation` permission and the required consent. Enable public-client flows for device-code login. Windows also needs the broker redirect URI `ms-appx-web://Microsoft.AAD.BrokerPlugin/CLIENT_ID`, with your app ID in place of `CLIENT_ID`. Tenant policy may block device-code login.

Check your session without opening sign-in UI:

```sh
devops auth status
```

Tokens renew silently when your session and tenant policy allow it. If a command exits with code `3`, open `devops tui` and sign in again. Windows and WSL keep separate sessions.

This is an early release. Live Windows/WSL sign-in and tenant workflows still need validation.

## Use the TUI

```sh
devops tui
```

| Key | Action |
| --- | --- |
| `p` | Pull requests |
| `i` | Assigned reviews needing attention |
| `l` | YAML pipelines |
| `s` | Sign-in and setup |
| `q` | Quit |
| Arrow keys, Enter | Select and open an item |
| `F5` | Refresh lists and details |
| `Esc` | Close a form or return home |

In PR details, use `e` to edit, `c` to comment, `v` to vote, `m` to merge, `a` to abandon, or `r` to reactivate. Press `n` in the PR list or details to create a PR. Merge and abandon ask for confirmation.

Forms use Tab to move between fields, Ctrl+U to clear a field, and Enter to submit.

For pipelines, choose a pipeline and then a run. Tasks appear on the left; the selected task's logs appear on the right. Arrow keys change selection and pause following. PageUp/PageDown scroll logs. Press `f` to resume following within the selected job.

The pipeline TUI only observes runs. Use the CLI to trigger, retry, cancel, or register pipelines.

## Use the CLI

Commands never prompt or open sign-in UI. Output is plain text with tab-separated tables; there is no JSON mode.

### Pull requests

```sh
devops pr list
devops pr status
devops pr view 123
devops pr create --repo my-repo --source feature --target main --title 'Fix build'
devops pr edit 123 --draft false --add-reviewer IDENTITY_UUID
devops pr comment 123 --body 'Reviewed the checks'
devops pr review 123 --vote approve
devops pr merge 123 --strategy squash --yes
devops pr abandon 123 --yes
devops pr reactivate 123
```

`pr status` lists active, non-draft PRs assigned directly to you with a review vote of zero or below.

Reviewer flags take Azure DevOps identity UUIDs, not email addresses or Entra object IDs. Repeat `--reviewer` when creating a PR to add several reviewers. Use `--remove-reviewer` when editing to remove one.

Votes are `approve`, `approve-with-suggestions`, `reset`, `wait`, and `reject`. Merge respects branch policies; it does not bypass them or delete the source branch.

### Pipelines

```sh
devops pipeline list
devops pipeline view 12
devops pipeline runs 12
devops pipeline run 12 --branch main --param deploy=false --variable mode=test
devops pipeline inspect 456
devops pipeline logs 456 --log 3 --start 0
devops pipeline watch 456
devops pipeline rerun 12 456
devops pipeline retry 456
devops pipeline cancel 456 --yes
devops pipeline register --name CI --repo my-repo --yaml /azure-pipelines.yml
```

Here `12` is a pipeline ID and `456` is a run ID. Registration points to YAML already committed in the repository; it does not create a YAML file.

Repeat `--param NAME=VALUE` for parameters. Values can be booleans, numbers, arrays, objects, or strings. `--variable NAME=VALUE` sets non-secret variables only. Never put secrets on the command line.

`rerun` starts a full new run with the previous run's parameters, non-secret variables, and pinned repository versions. It does not preserve skipped-stage choices and refuses secret overrides or non-repository resource inputs. `retry` asks the server to retry eligible failed work; you cannot select individual jobs.

`pipeline logs` returns up to 2,000 lines per call. Increase `--start` for the next page, or use the TUI for continuous logs. `pipeline runs` lists recent runs, not a complete history.

### Repositories

```sh
devops repo create new-repo
```

Creates an empty remote repository and prints its URL. No local Git changes or initial commit.

### Defaults and overrides

```sh
devops config
devops pr list --org another-org --project another-project
```

`--org` and `--project` override saved defaults for one command. They do not save changes or infer a target from Git remotes.

To save defaults without signing in or prompting:

```sh
devops init --org my-org --project my-project --tenant-id TENANT_UUID --client-id APP_UUID
```

Settings live in `~/.config/devops/settings.json`. On Windows that is `%USERPROFILE%\.config\devops\settings.json`, not AppData.

Linux/WSL stores an unencrypted session in `~/.config/devops/auth.json`, readable by your user and root. The directory is `0700` and the file is `0600`. Do not share this file. Windows stores account metadata in `wam-account.json`; WAM manages the credentials. `settings.json` never contains tokens.

### Writes and errors

Merge, abandon, and cancel require `--yes`. Trigger, rerun, and retry do not, even if the pipeline can deploy.

Writes are not retried automatically. If a request fails or is interrupted, check the resource before repeating it. A PR edit that changes several reviewers can partly succeed.

| Exit code | Meaning |
| --- | --- |
| `0` | Success |
| `1` | Network, service, or file error |
| `2` | Invalid arguments, missing settings, or missing `--yes` |
| `3` | Sign-in required |
| `4` | Permission or policy denied with HTTP 403 |
| `5` | Watched run failed or was canceled |

Use `devops --help` or `devops pr --help` for command options.

[Development and releases](docs/development.md) · [Acceptance checklist](docs/acceptance.md)
