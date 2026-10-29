# Live acceptance checklist

No live Azure DevOps writes have been run by the implementation session. Use an explicitly approved disposable repository, PR, and pipeline for these checks.

## 0.3.7 verification

Local Windows checks cover 90 Rust tests and five packaging tests, formatting,
and Clippy. Table rendering and PR detail tabs have automated coverage.
The credential-dependent live console test remains ignored; live tenant and
local WSL behavior were not retested for this patch.

## 0.3.6 verification

Windows checks cover 89 Rust tests, formatting, Clippy, five packaging tests, and
an optimized build. A local WebSocket test verifies subscription and console-line
delivery. Tests also cover task isolation, bounded retention, retries, handoff to
uploaded logs, error redaction, and quiet background refreshes.

A read-only check against sncom confirmed uploaded log retrieval for run 2992.
SignalR negotiation initially returned HTTP 401 with the user's scoped PAT. After
the user changed that PAT to full access, negotiation returned HTTP 200 and the
WebSocket WatchBuild subscription succeeded. No minimum required scope has been
established; full access is not a documented requirement of this application.
Run 2993 had already completed, so the opt-in live smoke test timed out waiting
for console batches. Incoming lines during an active tenant run remain unverified;
the user approved release with this limitation. No pipeline was triggered by this
session. Local WSL testing remains skipped at the user's request.

## 0.3.5 verification

Windows checks cover 83 Rust tests, formatting, Clippy, release packaging tests,
and the release build. Timer tests cover live updates, completed durations,
missing timestamps, clock skew, and both build and pipeline API date fields.
Timestamp tests cover relative times and UTC dates; a TUI rendering test checks
the duration column. Local WSL testing was skipped at the user's request.
Live tenant behavior was not retested for this patch.

## 0.3.4 verification

Windows checks cover 81 Rust tests, five packaging tests, formatting, Clippy,
and the release build. Loading-state tests cover animation, retaining rows during
refresh, and read cancellation. Tab navigation tests now verify the live pipeline
view opens by default. Its refresh interval is 16 seconds while visible.
Local WSL testing remains skipped at the user's request.

## 0.3.3 verification

Windows checks cover 79 Rust tests, five packaging tests, formatting, Clippy,
and the release build. New coverage verifies Markdown table escaping/alignment,
JSON array output, global flag placement, parseable configuration JSON, TUI flag
rejection, and write safeguards in JSON mode. Local WSL testing remains skipped.

## 0.3.2 verification

Windows verification passes 76 Rust tests, five release-packaging tests, formatting,
Clippy, and the release build. New tests cover grouped pipeline table selection,
project-wide run query limits and ordering, selection preservation when new runs
arrive, and the Settings menu. Recent runs refresh every 30 seconds while visible.
Local WSL testing remains skipped at the user's request; release CI checks Linux.

## 0.3.1 verification

Windows automated checks cover 72 Rust tests, formatting, Clippy, packaging, and
the release build. A Windows terminal check verified starred PAT entry,
Backspace, cancellation, and terminal restoration. A live read-only check against
sncom returned HTTP 200 with valid project-list JSON, both with and without redirect
suppression. The patched CLI login succeeded with the supplied environment PAT;
this did not reproduce the original HTTP 302 or verify every tenant operation.
Local WSL testing was skipped at the user's request; Linux remains covered by CI.

## 0.3.0 verification

Local Windows checks on 2026-10-28 cover formatting, Clippy, 65 Rust tests,
release packaging tests, and the release build. PAT tests cover current-user
DPAPI round trips, organization isolation, replacement, hidden TUI rendering,
Basic authentication, and stable authorization error codes using mocks.

Live PAT and Entra sign-in, WSL runtime behavior, and tenant operations remain
manual acceptance checks. Local WSL automated tests could not start because its
Cargo launcher could not find the underlying executable. Run CI on the release commit before tagging v0.3.0.
The historical verification records below describe earlier revisions.

## Automated verification

- macOS: the reviewed code passes 53 Rust tests, including device authorization polling, cache persistence, declined-login preservation, stale TUI responses, malformed response IDs, and token lifetime validation. Formatting, Clippy, and the release build pass. See [code review](code-review.md).
- Linux ARM64, Debian Bookworm container, Rust 1.88: all 45 tests pass. The existing Podman VM was started for this run and restored to its previous stopped state afterward. This is a Linux test, not a WSL session or live device-code login.
- Windows authentication/settings modules: cross-target type checking and Clippy pass for `x86_64-pc-windows-msvc`. The check used the real source modules in a temporary crate without the HTTP transport, avoiding a Windows C SDK dependency on macOS. This is not a full Windows application build or a WAM runtime test.
- macOS pseudo-terminal smoke: the actual binary rendered initial setup, accepted Escape and quit, exited successfully, restored terminal attributes/cursor, and left the alternate screen. The isolated test home contained no authentication cache. The smoke test did not contact a tenant.
- CI also checks the declared Rust 1.88 minimum on Linux. The CI workflows still need to run in a repository.
- Release packaging tests pass for Windows x64 and Linux x64/ARM64 archive layouts, executable permissions, attribution, and SHA-256 sidecars. Both workflow files pass actionlint. Publishing and mise installation still require a tagged GitHub workflow run; no release was published by this session.

## Authentication and setup

- PAT: run default `devops login --org <org> --project <project>` without environment variables or tenant/client IDs. Verify the hidden token prompt, successful project-listing validation, and subsequent use without reentering the token. Include Project and Team read plus operation-specific scopes. Confirm Windows cache files do not contain plaintext PATs; on Linux/WSL verify `0600` files and a `0700` directory.
- PAT: enter/paste a token in the TUI and confirm it renders only masked characters. Select the organization's project without profile/organization discovery; verify reads and cancel behavior. Check the optional `DEVOPS_PAT` override is not saved, normal commands never prompt, expired/revoked PATs return exit code 3, and insufficient scopes return 4. No tokens may appear in output or settings.
- Configure a tenant-owned public-client app with the Azure DevOps delegated permission, consent, public-client flows and Windows broker redirect URI.
- On native Windows, run `devops login --org <org> --client-id <app-id>` and verify WAM account selection, consent, cancellation, and terminal restoration.
- Verify tenant discovery from the organization, and check the fallback `--tenant-id` path.
- Restart the process and verify a normal CLI command acquires a token silently without UI.
- In WSL, complete `devops login` device-code sign-in. Verify `auth.json` is `0600`, its directory is `0700`, and `settings.json` contains no credentials.
- Restart WSL and verify the saved Linux session renews silently. Run simultaneous CLI/TUI requests near expiry and check refresh-token rotation.
- Revoke a session or require interaction through tenant policy. Verify CLI exit code 3 without a prompt, browser, or WAM UI.
- Select an organization and project from discovered lists. Verify persisted defaults, spaces in project names, and one-command overrides without settings changes. Verify `devops login --org ... --project ... --client-id ...` saves all three.

## Operations

- Create an empty remote repo and confirm no local Git changes or initial commit.
- List/view PRs, reviewer votes, status checks, branch policies, and discussion threads. Check the assigned-review attention filter.
- Create/edit a draft, publish it, edit title/description, add/remove reviewers, comment, and submit/reset a vote.
- Verify missing `--yes` prevents merge/abandon/cancel before authentication or network requests. Confirm the equivalent TUI merge/abandon dialog.
- Attempt merge with a blocked policy, then satisfy it and merge. Confirm the selected strategy and source-head guard, no bypass, and no branch deletion.
- Abandon/reactivate a disposable PR.
- Register a pipeline against existing committed YAML. Check server validation of nonexistent YAML and inaccessible repos.
- Trigger with a branch, typed parameters and permitted non-secret variable overrides. Verify protected overrides are denied by the server.
- Inspect/watch a run, including failure and cancellation exit codes. Exercise full rerun and server-side retry. Record the actual retry granularity.
- Observe sequential and parallel jobs. Select another job and resume following. Verify automatic selection never jumps to a parallel job.
- Scroll back during log updates; verify reading position, pagination, attempt changes, log retention, missing logs, reconnect behavior and final log delivery.
- Use queued runs with no timeline yet, deployment jobs, nested timelines, skipped stages, failed tasks and canceled runs. Record service-specific gaps before calling the viewer release-ready.
- Resize the TUI and quit during a pending read. Verify terminal restoration. If a write is interrupted, inspect server state rather than assuming cancellation.

## Release gate

- Run tests and Clippy on native Windows and Linux/WSL. CI definitions are supplied but have not been executed by this session.
- Keep live credentials and tenant response bodies out of diagnostics, screenshots, artifacts, and source control.
- Record tested tenant, OS/build, application registration settings, SDK version, and outcomes without tokens.
