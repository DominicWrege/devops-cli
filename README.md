# DevOps

Azure DevOps in your terminal. Review pull requests, watch pipeline logs, and run commands from scripts.

For Azure DevOps Services, not Azure infrastructure or self-hosted Azure DevOps Server. Supports YAML pipelines, pull requests, listing repositories, and creating empty remote repositories. No work items or classic pipelines.

## Install

Install a published release with [mise](https://mise.jdx.dev/):

```sh
mise use -g github:DominicWrege/devops-cli@0.3.9
devops login
```

Pin the version. `@latest` ignores releases less than 24 hours old, so it can report no versions just after a release. You can also download an archive from [Releases](https://github.com/DominicWrege/devops-cli/releases).

- Windows x64.
- Linux/WSL x64, a static musl binary with no glibc requirement.

Install separately on Windows and inside WSL. No ARM64 or macOS builds are provided. Azure CLI is not required.

## Sign in

Sign-in uses a personal access token by default (no Entra app registration). Entra sign-in remains optional. Azure CLI is not required for either option.

### Personal access token (PAT)

In Azure DevOps, open **User settings**, then **Personal access tokens**, then **New Token**. Select your organization, a short expiry, and only the scopes you need. Include **Project and Team: Read** for login and project selection, plus appropriate **Code** scopes for PRs/repositories and **Build** scopes for pipelines. Your organization must allow PATs.

Run login and enter your token at the hidden prompt:

```sh
devops login
# Or supply the target up front; the token is still prompted for:
devops login --org my-org --project my-project
```

Login verifies project-listing access, then saves the token separately from settings for subsequent CLI/TUI commands. On Windows the saved token is encrypted for your Windows user with DPAPI. Linux/WSL uses an owner-only file; see [storage](#defaults-and-overrides). Tokens are saved per organization, so overriding `--org` never silently uses another organization's saved PAT.

On first launch, `devops tui` asks for the organization and a masked token, then lets you select a project. A blank token field reuses the saved token. Organization/project selection is explicit; Git remote autodetection is not implemented.

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

```sh
devops tui
```

The TUI remembers your selected organization and project and opens directly to
pull requests on subsequent launches. If you are signed in but have no selected
project, it opens the project picker without asking for your token again.
From the main menu, press `s` for Settings, then choose Current project or Account. Project selection
is remembered for CLI commands too.

| Key | Action |
| --- | --- |
| `1` or `l` | Pipelines, starting with recent runs |
| `2` or `p` | Pull requests |
| `Tab` / `Shift+Tab` | Switch between pipelines and pull requests outside forms |
| `i` | Toggle assigned reviews needing attention from the PR list; open them from home |
| `s` | Settings from the main menu: current project and account |
| Up/Down, `Enter` | Select and open an item |
| `F6` | Focus another pane |
| `PageUp` / `PageDown` | Move through the focused list or scroll the focused pane |
| Left/Right | Pan long lines and table cells; switch tabs in PR details |
| `F5` | Refresh lists and details |
| `Esc` | Cancel a pending read, close a form, or leave the current view |
| `q` | Quit outside forms |

Loaded lists and activity stay cached for the TUI session and keep your selected
row. Press `F5` for fresh data. Refreshes keep existing rows visible, and an
animated indicator shows pending requests and elapsed time. `Tab` and `Shift+Tab`
can switch views while reads are loading. Submitted writes wait for server
confirmation before you can switch views.

Forms use `Tab` to move between fields, `Ctrl+U` to clear a field, and `Enter` to submit.

### Pull requests

Wide terminals show a preview beside the pull request list. `Enter` opens the selected PR.

PR details show a fixed header, description/comments/status tabs, and a review status sidebar on wide terminals. Use Left/Right arrows to switch detail tabs, `F6` to switch pane focus, and Up/Down or `PgUp`/`PgDn` to scroll. `[` and `]` also switch detail tabs. `Esc` returns to the PR list.

In PR details, press `v` to approve or `?` to open the actions menu for commenting, editing, merging, abandoning, reactivating, or creating a PR. The existing action shortcuts still work. To approve an assigned review, press `i` in the PR list, open the PR with `Enter`, then press `v`. Approval, merge, and abandon ask for confirmation. Press `n` in the PR list or details to create a PR.

### Pipelines and live logs

Pipelines opens the project-wide recent-runs feed by default. It lists the latest
50 runs from YAML pipelines and refreshes automatically while visible, keeping
your selected run. `Enter` opens its timeline and logs; `a` opens all pipelines.

The pipeline list groups rows by folder and sorts names within each folder.
Recent activity for the selected pipeline appears beside the list, or below it
on narrow terminals. Up/Down changes the pipeline; `Enter` opens its runs so you
can select one to follow. Activity loads after a short pause when scrolling past
pipelines. `F5` refreshes the list and activity; `r` returns to recent runs.

On the pipeline list, `e` opens the name and folder editor; `m` opens it with the
folder selected. Use `Tab` to switch fields, `Ctrl+U` to clear, `Enter` to save,
and `Esc` to cancel. A blank folder or `/` moves to the root. Failed saves retain
your edits; successful saves update the list and keep the pipeline selected.

In the run viewer, tasks appear on the left and the selected task's logs appear
on the right. `F6` cycles through tasks, logs, and summary. Up/Down and
`PageUp`/`PageDown` move through tasks or scroll the focused pane. Selecting a task
or scrolling logs pauses following; `f` resumes it within the selected job.
Status glyphs show success (✓), failure (✗), running (▶), canceled (■), and skipped (−).

Running tasks use a separate live-console connection. Uploaded logs are still polled
every two seconds and replace the live tail after the task completes. A new connection
may begin partway through a task; the uploaded log supplies its complete history.
Connection failures appear in the log pane and retry automatically without stopping
polling. Background timeline refreshes do not flash a loading indicator.

Use the CLI to trigger, rerun, retry, or cancel runs, or to register pipelines.

## Use the CLI

CLI commands other than `login` never prompt or open sign-in UI. Lists use bordered tables by default. Add `--json` to any CLI command
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
devops pipeline list --json
devops pipeline view 12
devops pipeline rename 12 'Build and test'
devops pipeline move 12 /Team/CI
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

`pipeline list` shows names and folders. Use `pipeline list --json` to retrieve
pipeline IDs for other commands. `pipeline move` accepts slash or backslash folder
paths; use `/` to move a pipeline to the root.

Repeat `--param NAME=VALUE` for parameters. Values can be booleans, numbers, arrays, objects, or strings. `--variable NAME=VALUE` sets non-secret variables only. Never put secrets on the command line.

`rerun` starts a full new run with the previous run's parameters, non-secret variables, and pinned repository versions. It does not preserve skipped-stage choices and refuses secret overrides or non-repository resource inputs. `retry` asks the server to retry eligible failed work; you cannot select individual jobs.

`pipeline logs` returns up to 2,000 lines per call. Increase `--start` for the next page, or use the TUI for continuous logs. `pipeline runs` lists recent runs, not a complete history.

### Repositories

```sh
devops repo list
devops repo list --json
devops repo create new-repo
```

`repo list` shows project repository names and clone URLs. Add `--json` for full
repository objects, including IDs. `repo create` creates an empty remote
repository and prints its URL. It makes no local Git changes or initial commit.

### Defaults and overrides

```sh
devops config
devops pr list --org another-org --project another-project
```

`--org` and `--project` override saved defaults for one command. They do not save changes or infer a target from Git remotes.

To save defaults without signing in:

```sh
devops init --org my-org --project my-project
devops login
```

For Entra defaults, add `--client-id APP_UUID` to `init`, then run
`devops login --entra`. `init` does not discover the tenant; Entra login does.
Add `--tenant-id` to either command only if discovery is unavailable.

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

[Development and releases](docs/development.md) | [Acceptance checklist](docs/acceptance.md)
