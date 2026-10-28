# DevOps CLI and TUI plan

Status: initial implementation in place. The user authorized implementation on 2026-10-27. Local build and mock tests cover the shared CLI/TUI workflows; live Windows/WSL and tenant acceptance remain open. See [README](../README.md) and [acceptance checklist](acceptance.md).

## Confirmed direction

- Latest authentication decision: use an interactive, hidden PAT prompt by default for now; keep Entra as an explicit alternative. See [ADR 0012](adr/0012-personal-access-token-authentication.md). The WAM/device-code requirements below apply to Entra mode.
- Next requested feature: repository autodetection. It is deferred to the next implementation pass; organization/project selection remains explicit in this change.
- Build both a command-line interface and a terminal user interface in Rust.
- Build the TUI for personal daily use by the project owner and the CLI for agents. Team distribution is not a confirmed requirement.
- Support agents running natively on Windows and inside WSL on the same PC. Remote hosts and CI are not first-release requirements. Linux/WSL uses its own device-code session rather than sharing the Windows WAM session.
- Use WAM for native Windows and Microsoft Entra OAuth 2.0 device-code login for Linux/WSL. Do not require a Windows executable or broker bridge for Linux/WSL. See [ADR 0009](adr/0009-linux-device-code-authentication.md).
- Treat the TUI as the human interface and the CLI as the agent interface. See [ADR 0003](adr/0003-tui-for-humans-cli-for-agents.md).
- CLI output is text only. Do not add JSON output or a `--json` flag. Except for the explicit interactive `login` command, CLI commands never prompt. See [ADR 0005](adr/0005-text-only-cli-output.md), which supersedes ADR 0004.
- Ship one binary with an explicit `tui` subcommand. CLI commands do not launch the TUI. See [ADR 0001](adr/0001-cli-and-tui-entry-points.md).
- Target Azure DevOps. Azure infrastructure management is outside the current scope.
- Support Microsoft-hosted Azure DevOps Services in the first release. Self-hosted Azure DevOps Server is outside the current scope.
- Include pull requests, pipelines, and repository creation in the first-release scope. Work items are deferred to a later release. Other repository operations are not decided yet.
- Repository creation and the PR and pipeline write operations listed below are confirmed.
- The first identified PR workflow is finding pull requests that need attention. The CLI `pr status` and TUI assigned-review filter implement the initial direct-reviewer definition of attention.
- Use `azure_devops_rust_api` for Azure DevOps operations, with other Azure SDK crates as needed. The user chose this despite its documented deprecation. Verify version compatibility and workflow coverage before implementation. See [ADR 0006](adr/0006-use-azure-devops-rust-api.md).
- Use native Windows Web Account Manager, WAM, to reuse the user's existing Windows Entra sign-in. The user explicitly confirmed this choice. The TUI handles any required human interaction; the agent CLI acquires tokens silently. App registration, consent, and the Azure DevOps SDK adaptation remain to be verified. See [ADR 0007](adr/0007-built-in-entra-sign-in.md).
- Do not depend on Azure CLI, `az login`, or a Python runtime. The user rejected that authentication route. Keep built-in sign-in as the requirement.
- Require silent token renewal when the Entra session permits it. If human interaction is required, the CLI fails without prompting. Use the user-provided [az-mise-plugin](https://github.com/SNcom-GmbH/az-mise-plugin) as the authentication design reference: native Windows WAM through `windows`, exposed as an Azure SDK `TokenCredential`. The user reports this reference works. Adaptation to Azure DevOps still needs validation. See [authentication research](authentication-research.md).
- For Linux/WSL, use the Rust `oauth2` crate and an Azure SDK `TokenCredential` adapter for device-code login and silent renewal. The TUI handles initial login. Persist the session in `~/.config/devops/auth.json`, separate from `settings.json`, using directory mode `0700` and file mode `0600`. This cache is unencrypted at rest. Use atomic writes and a file lock for refresh coordination; never log tokens. App registration still needs verification.
- Keep Linux/WSL authentication simple: no keyring daemon, kernel-keyring dependency, encrypted vault, background authentication service, or Windows bridge.
- The requirements interview is complete for this implementation pass. Preserve the recorded decisions while implementing.
- Use GitHub CLI, `gh`, as the CLI design reference, not as a feature-parity requirement. See [ADR 0002](adr/0002-use-gh-as-cli-reference.md).
- During TUI setup, discover accessible Azure DevOps organizations, let the user select one, then query its projects and save the chosen organization/project defaults in `~/.config/devops/settings.json`. Resolve `~` to the user's home directory, including on Windows; do not substitute AppData. The `devops` directory name is provisional until naming is settled. See [ADR 0008](adr/0008-user-project-settings.md).
- Resolve organization/project from settings unless explicitly overridden by flags. Overrides apply only to that invocation and never modify saved settings. Do not infer a different target from the current Git remote.
- Support interactive setup through both `devops login` and the TUI. The CLI `init` command accepts explicit arguments and never prompts.
- Confirm high-impact writes only: merging/completing or abandoning a PR and canceling a pipeline run. Require TUI confirmation where the action is available and an explicit acknowledgement flag in the non-interactive CLI. Routine creates, edits, comments, and votes need no extra confirmation. Explicit trigger, rerun, and retry commands are sufficient without an extra acknowledgement flag, including for pipelines that can deploy. See [ADR 0010](adr/0010-high-impact-write-confirmation.md).

## Remaining checks

- SDK request builders cover organization discovery, project listing, PRs, timelines/logs, and repositories. Mock tests verify payloads and paths, including model repairs described in [ADR 0011](adr/0011-sdk-request-compatibility.md). Actual tenant behavior and retry eligibility still need live checks. Job-level retries are not promised.
- Verify app registration, Azure DevOps token audience, delegated permissions, tenant consent, WAM behavior, and Linux device-code policy in the target tenant.
- Implemented command inputs, tab-separated text tables, labeled detail output, stderr errors, exit codes, and `--yes` acknowledgement are documented in the README. Validate these contracts during acceptance.
- The PR TUI uses action forms and explicit high-impact confirmation. The pipeline TUI uses a stage/job/task sidebar, two-second polling, task-local logs, explicit `f` to resume following, and bounded retention. Test usability and deployment-job timeline coverage against a tenant.
- Settle the final binary name and release packaging. These do not change the agreed feature scope.

## First-release operations

### Pull requests

- List, view, and status, including PRs needing attention, reviewer votes, and checks.
- Create and edit, including titles, descriptions, draft state, and reviewers.
- Comment and review, including discussion threads and review votes.
- Complete or merge using an allowed strategy, subject to Azure DevOps policies.
- Abandon and reactivate.

Diff and local checkout are not first-release requirements. Initial inputs and safeguards are implemented; command contracts are in the README and live acceptance cases are in `docs/acceptance.md`.

The PR TUI supports all selected PR actions, including creation, editing, comments, review votes, completion/merge, abandonment, and reactivation. This differs from the observation-only pipeline TUI. Action forms and high-impact confirmation are implemented; usability still needs human acceptance.

### Pipelines

Support YAML-defined build and deployment pipelines. Classic build and classic release pipelines are outside the first-release scope.

- List definitions and runs; inspect status, stages, jobs, and logs.
- Trigger runs with a selected branch, parameters, and permitted variable overrides.
- Watch runs until completion.
- Rerun or retry. Verify server and SDK support before promising job-level retry or defining exactly which prior-run inputs are reused.
- Cancel runs.
- Register a pipeline definition pointing to YAML already committed in a repository. YAML generation or editing is not part of this operation.

Initial command contracts and SDK coverage are implemented. Variable overrides are non-secret only; the server enforces override permissions. Actual tenant behavior remains an acceptance check.

The pipeline TUI is observation-only. It shows live run progress, a task sidebar on the left, and logs in the main pane, similar to Azure DevOps' web UI. Follow execution within the selected job until the user chooses another task or scrolls back; resume only on explicit request. Trigger, rerun/retry, cancel, and registration operations stay in the CLI. See [pipeline TUI design](tui-pipeline.md) for implemented hierarchy and update defaults.

### Repositories

Create an empty remote Azure DevOps Git repository and return its URL. Do not create an initial commit, README, local clone, or local checkout as part of first-release repository creation.

### Initial setup

The TUI setup wizard discovers organizations, lets the user select one, queries its projects, and saves organization/project defaults in `~/.config/devops/settings.json`. The CLI setup equivalent requires explicit arguments and never prompts.

Regular CLI `--org` and `--project` flags override settings only for the current invocation. They do not save a new default. Updating saved defaults is an explicit setup/configuration action.

## Deferred scope

- Work items, including tasks, bugs, and stories. Revisit their operations and interface design after the first release.

## Implementation choices

- Share SDK access and workflow logic in `src/service.rs`; both interfaces use the same authentication adapters and settings resolution.
- Use resource/action commands such as `pr list`, `pipeline run`, and `repo create`. `devops` remains a provisional binary name.
- Do not infer repository context from Git or launch browsers from CLI commands. Browser-link commands are not added.
- Keep all prompts in the TUI. CLI output and exit codes are documented in the README and covered by process-level tests.

## Implementation sequence

1. Verify SDK and authentication feasibility for the agreed workflows. Document any missing API support before changing scope or dependencies.
2. Build the shared application layer, settings resolution, text-only non-interactive CLI foundation, authentication adapters, and the TUI setup wizard.
3. Implement repository creation and the selected PR operations, including the full-action PR TUI and write confirmations.
4. Implement YAML pipeline CLI operations, then the observation-only pipeline TUI with task selection, incremental logs, and pause/resume following.
5. Complete automated tests, verify Windows and WSL behavior in the target tenant, and package the first release.

The user authorized this sequence. Stages 1-4 have an initial implementation with mock coverage. Stage 5 has local checks and Windows/Linux CI definitions; native Windows/WSL execution, tenant validation, and final release approval remain open.

Local verification on macOS completed on 2026-10-27: `cargo test --locked` passed 43 tests, `cargo clippy --locked --all-targets -- -D warnings` and formatting checks passed, and `cargo build --locked --release` succeeded. The release binary's help command also ran successfully. No live tenant requests or writes were used for these checks.

Follow-up validation added initial device-code polling and declined-login preservation tests. All 45 tests now pass on macOS and Linux ARM64 using Rust 1.88. The real Windows authentication/settings modules pass cross-target type checking and Clippy, but a full native Windows build and WAM runtime check are still required. A macOS pseudo-terminal smoke verified setup rendering, navigation/quit, cursor and terminal-state restoration, with no live sign-in or credential writes. See `docs/acceptance.md` for the exact validation boundaries.

The subsequent [code review](code-review.md) removed Rust `unwrap`/`expect` panic calls from application code and tests, added lint guards, reused PR snapshots and TUI initialization/run-loading helpers, and fixed stale replies, response-ID truncation, and token-expiry overflow. The updated suite passes 53 Rust tests on macOS plus two packaging tests. The required WAM interop remains the sole unsafe-code exception, with safety comments and a Windows cross-target Clippy check.

## Acceptance checks

- Human setup discovers organizations and projects and persists the selected defaults at the agreed home-config path.
- Explicit context flags override saved defaults without changing the settings file. Missing required context fails without prompting; the current directory does not silently change the target.
- Windows reuses the Entra account through WAM. Linux/WSL completes device-code login in `devops login` or the TUI, persists its owner-only cache, and renews silently across CLI invocations.
- Except for `devops login`, CLI commands never prompt, open authentication UI, or launch the TUI. Authentication requiring human interaction fails with a clear instruction to run `devops login`.
- Credentials never appear in settings, command output, or diagnostic logs. Linux cache writes are atomic, owner-only, and coordinated between processes.
- All selected PR operations work through the CLI and TUI. Merge and abandonment follow the accepted acknowledgement/confirmation rules and respect server permissions and policies.
- The selected YAML pipeline CLI operations work, with retry behavior limited to verified service/SDK support. Trigger/rerun/retry need no extra acknowledgement; cancellation does.
- The pipeline TUI shows progress and left-sidebar tasks with main-pane logs, follows within the selected job, pauses on manual navigation, and resumes only on request. It exposes no pipeline write controls.
- Repository creation produces an empty remote Git repository and returns its URL without touching local Git state.
- Work items, classic pipelines, local PR checkout/diff, JSON CLI output, Azure CLI, Python, and extra authentication services are not required for the first release.
- Use mocks for automated API/error-path tests. Run any live write acceptance tests only against explicitly approved disposable resources.

## SDK feasibility

- Microsoft's `azure-devops-rust-api` repository currently declares the project deprecated, unmaintained, and unsupported. See [SDK research](sdk-research.md).
- The implementation uses `azure_devops_rust_api` 0.39.1 rather than a handwritten REST client, with selected resource features. Compatibility policies repair generated SDK gaps; see ADR 0011.
- The supplied `azure_core` 1.1 and `azure_identity` 1 dependencies compile together with the DevOps SDK. HTTPS uses rustls, with an explicit ring provider at startup. The lockfile records resolved dependencies. `azure_identity` remains in the dependency set as requested; the selected public-client flows use custom SDK credential adapters, not Azure CLI credentials.

## Documentation approach

- Update this plan as answers narrow the scope.
- Record architectural decisions in `docs/adr/` once we make them.
- Maintain `docs/glossary.md` as domain terms become clear.
- Keep proposals and unanswered questions separate from accepted decisions.

## Implementation gate

Implementation was authorized after the requirements interview. Release readiness still requires the live checks in `docs/acceptance.md`; passing local mock tests is not proof of tenant or Windows broker compatibility.
