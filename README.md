# DevOps

Rust Azure DevOps Services client. Agents use a non-interactive, text-only CLI. Humans use `devops tui`.

This is an initial implementation. Live Windows WAM, WSL login, and Azure DevOps tenant acceptance tests are still required. The binary name is provisional.

## Install with mise

Once a tagged GitHub release is published, install with [mise](https://mise.jdx.dev/):

```sh
mise use -g github:DominicWrege/devops-cli@latest
devops tui
```

To pin a release, use `@0.1.0` instead of `@latest`. Private repository installs require GitHub credentials available to mise. No custom mise plugin, Rust compiler, Python runtime, or Azure CLI is needed to use the prebuilt binary.

Release archives support Windows x64 and Linux/WSL x64 and ARM64. Linux x64 requires glibc 2.35 or newer, ARM64 requires glibc 2.39 or newer. Alpine/musl and macOS release builds are not provided. Windows and WSL must install separately. Each archive includes the WAM attribution notice and a SHA-256 checksum file is published alongside it.

### Publish a release

Push the project to GitHub first. Set `package.version` in `Cargo.toml`, update `Cargo.lock`, then push a matching tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release.yml` checks the tag against the manifest, tests and builds on each native platform, packages the binaries, verifies checksums, then publishes a GitHub release. It uses the repository's `GITHUB_TOKEN`, with write permission only in the publishing job. It then smoke-tests installation with mise on each platform. Tags such as `v0.2.0-rc.1` become prereleases, installable with `mise use -g 'github:DominicWrege/devops-cli[prerelease=true]@0.2.0-rc.1'`.

If publication fails after creating the draft, inspect that draft before retrying. The workflow does not overwrite an existing release or its assets. CI builds and install smoke tests do not verify live Entra sign-in. See [the acceptance checklist](docs/acceptance.md).

## Build and run

Requires Rust 1.88 or newer and a native build toolchain.

```sh
cargo build --locked --release
cargo run -- tui
```

Use a native Windows build on Windows and a Linux build inside WSL. The macOS build is used for development, not a tested release target.

## Sign in and choose defaults

Open `devops tui`, then press `s` to configure your tenant UUID and your own Entra public-client application UUID. The first launch opens this form automatically. There is no bundled application ID and no Azure CLI dependency.

Your registration needs Azure DevOps delegated `user_impersonation` permission and any consent your tenant requires. Enable public-client flows for Linux device-code login. Windows broker sign-in also needs WAM-compatible registration, including the broker redirect URI `ms-appx-web://Microsoft.AAD.BrokerPlugin/CLIENT_ID`. Verify this configuration with your tenant administrator. Tenant policy may prohibit device codes or require another human sign-in.

Windows uses native WAM, first trying the existing Windows account silently. Only the TUI can open broker UI. Linux/WSL displays a device code in the TUI and caches the resulting session for silent renewal.

After sign-in, select an accessible organization, then a default project. Configuration lives in:

- `~/.config/devops/settings.json`, organization/project and app identity, never tokens.
- Linux/WSL `~/.config/devops/auth.json`, an unencrypted session cache. Directory permissions are `0700`, file permissions `0600`. Refresh operations use a file lock and atomic replacement. Same-user processes and root can read these credentials.
- Windows `~/.config/devops/wam-account.json`, selected account metadata only. WAM owns token persistence.

On Windows, `~` is `%USERPROFILE%`, not AppData. Windows and WSL have separate settings and sessions. Nothing automatically shares them.

An explicit-argument setup command saves defaults without signing in or prompting:

```sh
devops init --org my-org --project my-project --tenant-id TENANT_UUID --client-id APP_UUID
devops config
devops auth status
```

Normal `--org` and `--project` flags override defaults for one command only. They never save or infer a target from Git remotes.

## CLI

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

devops repo create new-repo

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

`--reviewer`, `--add-reviewer`, and `--remove-reviewer` use Azure DevOps identity UUIDs, not Entra object IDs or email addresses. Create accepts repeatable `--reviewer`. Review votes are `approve`, `approve-with-suggestions`, `reset`, `wait`, and `reject`.

Only merge, abandon, and cancel need `--yes`. Trigger, rerun, and retry can deploy through YAML and do not require extra acknowledgement, as selected in the plan. Azure DevOps permissions and policies always apply. Merge sends the observed source commit and never requests policy bypass or source-branch deletion.

Repository creation makes an empty remote Git repository. It does not create commits or alter local Git state. Pipeline registration references existing committed YAML; it does not generate YAML.

Parameters accept repeatable `NAME=VALUE`. Valid typed literals become booleans, numbers, arrays, or objects; other values become strings. Variables are non-secret overrides only. Do not put secrets on the command line. Keep secrets in protected Azure DevOps variables or secret stores. The server decides which overrides are permitted.

CLI tables have a header and tab-separated columns. Details use labeled text. Output goes to stdout, errors to stderr. No JSON output, prompts, automatic browser launch, or implicit TUI launch. Control characters from remote content are stripped. No automatic write retries are enabled. If a connection fails during a write, inspect the resource before repeating it. Multi-request PR reviewer edits can partly succeed.

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | Network, service, file, or other operation failure |
| 2 | Invalid arguments, missing context/configuration, or missing acknowledgement |
| 3 | Human sign-in required |
| 4 | Permission/policy denial reported as HTTP 403 |
| 5 | Watched run finished unsuccessfully, including cancellation |

## TUI

Home keys are `p` for PRs, `i` for assigned reviews needing attention, `l` for YAML pipelines, `s` for setup, and `q` to quit. In the PR list, `i` toggles the assigned-review filter. Attention means a non-draft active PR assigned directly to your identity with a review vote of zero or below. `Esc` returns home. Lists use arrow keys and Enter. `F5` refreshes lists/details. Forms use Tab to select fields, Ctrl+U to clear a field, Enter to submit, and Esc to cancel.

PR details expose create, edit, comment, review, merge, abandon, and reactivate. Merge and abandon require a separate `y` confirmation.

The pipeline screen is observation-only. Choose a pipeline, then a run:

- Left sidebar shows stages, jobs, and tasks with status text.
- Main pane shows the selected task's logs, polled every two seconds.
- Following stays within the selected job. Up/Down changes selection and pauses following. PageUp/PageDown scrolls logs and also pauses it. Press `f` to resume.
- Each log fetch reads at most 2,000 lines. The view retains at most 10,000 lines or 2 MiB and truncates single lines over 64 KiB. The title shows the first retained line offset.
- Network work does not block input. Failures preserve the last view and retry on the next poll. Terminal state is restored on normal exit and errors.

## Current limits and verification

- YAML pipelines only, no classic builds/releases or work items.
- `pipeline runs` shows the service's recent run list, currently up to 10,000 runs. It is not an unlimited historical export.
- `pipeline logs` returns one page of up to 2,000 lines. Use `--start` for the next page. The TUI advances offsets automatically.
- `rerun` creates a new full run using the prior returned template parameters, non-secret variables, and pinned repository versions. It does not reproduce a prior stage-skip selection. It refuses secret overrides and non-repository resource inputs rather than silently losing them.
- `retry` calls the SDK's build update with `retry=true`. Eligible failed work depends on the server. Job-level selection is not implemented or promised; tenant behavior still needs validation.
- Organization discovery requires a provisioned Azure DevOps profile and accessible organizations. Setup selects existing organizations/projects; it does not create them.
- The selected SDK is deprecated upstream. Request policies repair missing YAML-registration fields, PR draft/expected-head fields, sparse build updates, and leading doubled URL slashes. Requests still use `azure_devops_rust_api`.

Run `cargo test --locked` and `cargo clippy --locked --all-targets -- -D warnings`. The reviewed code passes 53 Rust tests on macOS, plus two release-packaging tests. The earlier 45-test suite also passed on Linux ARM64 with Rust 1.88. Automated tests use local mock HTTP servers, not a real tenant. The Windows authentication/settings modules also pass cross-target checking; native Windows execution remains untested. CI definitions cover native Windows/Linux and the Rust 1.88 minimum. Run the [manual acceptance checklist](docs/acceptance.md) on Windows and WSL before using this against important resources.

The application and tests deny Rust `unwrap`/`expect` panic calls. Application lints also reject explicit panic/placeholders and undocumented unsafe blocks. Unsafe code is denied everywhere except the native Windows WAM module, whose five Windows interop calls have safety comments. These rules apply to this project's code, not its dependencies. See [the review record](docs/code-review.md).
