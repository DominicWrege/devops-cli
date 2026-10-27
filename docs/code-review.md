# Code review, 2026-10-27

Reviewed the first-party Rust modules, CLI process tests, and release packaging/workflows before the initial Git push. This was not an audit of dependency internals or a live tenant test.

## Fixes

- Removed Rust `unwrap`, `unwrap_err`, `expect`, and explicit panic/unreachable calls from application code and tests. Errors now propagate with context. Wiremock's `expect(1)` methods are request-count assertions, not Rust panic calls.
- Added crate-level Clippy guards against those panic calls, placeholders, and undocumented unsafe blocks. Tests use `Result` rather than panicking during setup.
- Reused command validation for positive IDs and repository names. Repository creation now validates before authentication. Removed redundant reviewer validation inside already-validated actions.
- Made parameter parsing return a JSON map directly instead of unwrapping a generic JSON value.
- Reused one PR snapshot for TUI data and details rather than fetching the same PR twice.
- Extracted shared TUI initialization and pipeline-run loading, also used by tests and refresh.
- Tagged asynchronous replies with request generations. Late replies from an aborted viewer cannot update another run, clear a new task, or replace its status.
- Checked remote IDs before converting to 32-bit resource IDs. Oversized IDs no longer wrap to another PR, pipeline, run, or log.
- Checked token lifetimes with a shared expiry helper. Invalid lifetimes return errors instead of overflowing arithmetic.
- Replaced SDK policy-chain indexing and form-field indexing with checked access and errors.

## Unsafe boundary

`src/main.rs` denies unsafe code. `src/auth/mod.rs` permits it only in `src/auth/wam.rs`, which contains five Windows API calls requiring unsafe bindings:

- Initialize and uninitialize the Windows Runtime on the same blocking thread.
- Obtain the console window and, when necessary, the foreground window.
- Request an interactive WAM token for that window.

Each call has a safety comment. Initialization has a scope guard; COM objects are dropped before uninitialization. Interactive requests run only during TUI setup and keep request/interop objects alive until completion. A literal zero-unsafe requirement would conflict with the current native WAM design. This review does not claim that the dependency tree contains no unsafe code.

## Checks

- `cargo test --locked`: 53 tests pass on macOS.
- `cargo clippy --locked --all-targets -- -D warnings`: passes.
- `cargo fmt --all -- --check`: passes.
- `cargo build --locked --release`: passes.
- Release packaging: two Python test methods pass across all three archive targets.
- Both GitHub Actions workflows pass actionlint.
- The actual Windows authentication/settings source passes cross-target Clippy with unwrap/expect and unsafe-documentation lints enabled in the transport-free validation crate described in `acceptance.md`.

Native Windows execution, live sign-in, and tenant API acceptance remain open. The earlier Linux 45-test result predates these review changes; the pushed CI must check the updated code on Linux and Windows.
