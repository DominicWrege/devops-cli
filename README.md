# DevOps

Azure DevOps in your terminal. Review pull requests, watch pipeline logs, and run commands from scripts.

For Azure DevOps Services, not Azure infrastructure or self-hosted Azure DevOps Server. Supports YAML pipelines, pull requests, and empty remote repository creation. No work items or classic pipelines.

## Install

Install a published release with [mise](https://mise.jdx.dev/):

```sh
mise use -g github:DominicWrege/devops-cli@0.3.6
devops login
```

Pin the version. `@latest` ignores releases less than 24 hours old, so it can report no versions just after a release. You can also download an archive from [Releases](https://github.com/DominicWrege/devops-cli/releases).

- Windows x64.
- Linux/WSL x64, a static musl binary with no glibc requirement.

Install separately on Windows and inside WSL. No ARM64 or macOS builds are provided. Azure CLI is not required.

## Sign in

Sign-in uses a personal access token by default (no Entra app registration). Entra sign-in remains optional. Azure CLI is not required for either option.

### Personal access token (PAT)

In Azure DevOps, open **User settings â†’ Personal access tokens â†’ New Token**. Select your organization, a short expiry, and only the scopes you need. Include **Project and Team: Read** for login and project selection, plus appropriate **Code** scopes for PRs/repositories and **Build** scopes for pipelines. Your organization must allow PATs.

Run login and enter your token at the hidden prompt:

```sh
devops login
# Or supply the target up front; the token is still prompted for:
devops login --org my-org --project my-project
```

Login verifies project-listing access, then saves the token separately from settings for subsequent CLI/TUI commands. On Windows the saved token is encrypted for your Windows user with DPAPI. Linux/WSL uses an owner-only file; see [storage](#defaults-and-overrides). Tokens are saved per organization, so overriding `--org` never silently uses another organization's saved PAT.

`devops tui` also asks for the organization and a masked token, then lets you select a project. A blank token field reuses the saved token. Organization/project selection is explicit for now; repository autodetection is the next feature, not implemented yet.

For scripts, `DEVOPS_PAT` is an optional override that takes precedence over saved credentials and is never saved by login. Set it through a secret manager or hidden shell input, not a literal token in shell history. Environment variables are available to child processes; do not log or share them. Never pass the token as a command-line argument. Non-interactive commands never prompt for tokens.

PATs do not renew automatically. Run `devops login` again to enter a replacement when the token expires or is revoked. To save defaults without signing in, use `devops init --org my-org --project my-project` (`--pat` is optional). Your organization must allow PATs.

### Microsoft Entra

```sh
devops login --entra
```

With `--entra`, the first run asks for your organization and Entra public-client app ID. It discovers the tenant from the organization, then completes sign-in. Later Entra logins reuse the saved values. Windows uses your Windows account through WAM. Linux/WSL shows a device code and a URL to open. Unset `DEVOPS_PAT` before switching to Entra.

Pass the values up front instead:

```sh
devops login --org my-org --project my-project --client-id APP_UUID
```

The organization is the first path segment of your Azure DevOps URL: `https://dev.azure.com/my-org/my-project`. For Entra sign-in, ask your tenant administrator for a public-client app registration with Azure DevOps delegated `user_impersonation` permission and consent. Enable public-client flows for device-code login. Windows also needs the broker redirect URI `ms-appx-web://Microsoft.AAD.BrokerPlugin/CLIENT_ID`, with your app ID in place of `CLIENT_ID`. Tenant policy may block device-code login.

`devops tui` offers the same setup interactively. Press `s` there and select Account to reopen sign-in later.

Entra tokens renew silently when your session and tenant policy allow it. If a command exits with code `3`, run `devops login --entra` again (or `devops login` to replace a PAT). Windows and WSL keep separate sessions.

This is an early release. Live Windows/WSL sign-in and tenant workflows still need validation.

## Use the TUI

The TUI remembers your selected organization and project and opens directly to
pull requests on subsequent launches. If you are signed in but have no selected
project, it opens the project picker without asking for your token again.
Press `s` for Settings, then choose Current project or Account. Project selection
is remembered for CLI commands too.
Use `1` / `2` or `Tab` to switch between pull requests and pipelines. Wide terminals
show a preview beside the pull request list. `Enter` opens the selected item.
Pipelines opens the live project-wide recent-runs feed by default. It lists the latest
50 runs from YAML pipelines and refreshes every 16 seconds while visible, keeping
your selected run. Enter opens its timeline and logs; `a` returns to all pipelines.

The pipeline table groups rows by folder and sorts names within each folder.
The Pipelines view shows recent activity for the selected pipeline beside the list,
or below it on narrow terminals. Up/Down changes the pipeline; Enter opens its
runs so you can select one to follow. F5 refreshes activity. Tab and Shift+Tab
switch views even while reads are loading; submitted writes finish first.
Use `F6` to focus another pane. Up/Down and PageUp/PageDown scroll the focused
pane; Left/Right pan long lines and table cells. In the run viewer, F6 cycles
through tasks, logs, and summary. Scrolling logs pauses follow; `f` resumes it.
Status glyphs show success (✓), failure (✗), running (▶), canceled (■), and skipped (−).

An animated indicator shows pending requests and elapsed time. Refreshes keep
existing rows visible; Esc cancels a pending read. Submitted writes wait for server
confirmation.
Loaded lists and activity stay cached for this TUI session, so returning to a tab
is immediate and keeps your selected row. Press F5 for fresh data. Activity loads
after a short pause when scrolling past pipelines. Run icons distinguish running,
succeeded, failed, and canceled runs; the text status remains visible.


```sh
devops tui
```

| Key | Action |
| --- | --- |
| `p` | Pull requests |
| `i` | Assigned reviews needing attention |
| `l` | YAML pipelines |
| `s` | Settings: current project and account |
| `q` | Quit |
| Arrow keys, Enter | Select and open an item |
| `F5` | Refresh lists and details |
| `Esc` | Close a form or return home |

In PR details, use `e` to edit, `c` to comment, `v` to vote, `m` to merge, `a` to abandon, or `r` to reactivate. Press `n` in the PR list or details to create a PR. Merge and abandon ask for confirmation.

Forms use Tab to move between fields, Ctrl+U to clear a field, and Enter to submit.

For pipelines, choose a pipeline and then a run. Tasks appear on the left; the selected task's logs appear on the right. Arrow keys change selection and pause following. PageUp/PageDown scroll logs. Press `f` to resume following within the selected job.

Running tasks use a separate live-console connection. Uploaded logs are still polled
every two seconds and replace the live tail after the task completes. A new connection
may begin partway through a task; the uploaded log supplies its complete history.
Connection failures appear in the log pane and retry automatically without stopping
polling. Background timeline refreshes do not flash a loading indicator.

The pipeline TUI only observes runs. Use the CLI to trigger, retry, cancel, or register pipelines.

## Use the CLI

Commands other than `login` never prompt or open sign-in UI. Lists use aligned Markdown-style tables by default. Add `--json` to any CLI command
for machine-readable output, for example `devops pipeline list --json` or
`devops --json pr list`. Lists return arrays; resource views return objects;
`pipeline inspect` returns `run` and `timeline`; logs return an array of lines.
Action confirmations and login/init return a `message` object. `pipeline watch --json`
emits one JSON run object per update on separate lines. Errors remain on stderr
with the existing exit codes. The interactive TUI does not accept `--json`.

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

To save defaults without signing in:

```sh
devops init --org my-org --project my-project --client-id APP_UUID
devops login
```

`init` does not discover the tenant. `login` does that when it runs. Add `--tenant-id` to either command only if discovery is unavailable.

Settings live in `~/.config/devops/settings.json`. On Windows that is `%USERPROFILE%\.config\devops\settings.json`, not AppData.

PATs entered interactively are saved in `~/.config/devops/pat-ORGANIZATION.json`, separate from settings. Windows encrypts the token with current-user DPAPI. Linux/WSL stores it unencrypted with directory mode `0700` and file mode `0600`, readable by your user and root. Do not share these files. `DEVOPS_PAT` overrides are never saved.

For Entra sign-in, Linux/WSL stores an unencrypted session in `~/.config/devops/auth.json` with the same owner-only permissions. Windows stores account metadata in `wam-account.json`; WAM manages the credentials. `settings.json` never contains tokens.

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

[Development and releases](docs/development.md) Â· [Acceptance checklist](docs/acceptance.md)
