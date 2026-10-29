# Development and releases

## Build

Requires Rust 1.88 or newer and a native build toolchain.

```sh
cargo build --locked --release
cargo run -- tui
```

Use a native Windows build on Windows and a Linux build inside WSL. macOS is used for development, not as a release target.

Release targets are Windows x64 MSVC and Linux x64 musl only. ARM64 release builds have been removed.

For the static Linux build, install `musl-tools` and the Rust `x86_64-unknown-linux-musl` target. CI explicitly enables `crt-static`, compiles C dependencies with `musl-gcc`, and links the executable with Rust's bundled LLD. Ubuntu's `musl-gcc` wrapper is not used for the final static-PIE link. The shared setup action checks a small executable's startup before building the application. CI checks the finished ELF for an interpreter and shared-library dependencies, then runs the binary in Alpine. Ordinary CI also checks this musl build before a tag is pushed.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```

Automated API tests use local mock servers, not a tenant. See the [review record](code-review.md) for code checks and the [acceptance checklist](acceptance.md) for Windows/WSL sign-in and live API validation.

The application and tests deny Rust `unwrap`/`expect` panic calls. Application lints reject explicit panic/placeholders and undocumented unsafe blocks. Unsafe code is denied except in the native Windows WAM and DPAPI modules. Their interop calls and buffer ownership have safety comments. These rules apply to first-party code, not dependencies.

The Azure DevOps SDK is deprecated upstream. Small request policies repair missing request fields and URL paths without replacing the SDK client. See [SDK research](sdk-research.md) and [ADR 0011](adr/0011-sdk-request-compatibility.md).

## Publish a release

Set `package.version` in `Cargo.toml`, update `Cargo.lock`, and commit. Push the
commit and wait for Windows, Linux musl, packaging, and Rust 1.88 CI to pass.
Complete the live acceptance checklist before pushing a matching release tag:

```sh
git tag v0.3.5
git push origin v0.3.5
```

`.github/workflows/release.yml` validates the tag, tests and builds on each native platform, packages the binaries, verifies SHA-256 checksums, and publishes a GitHub release. The publishing job alone has write permission. Each archive includes the WAM attribution notice. A final job installs the published release through mise on each platform.

Tags such as `v0.3.5-rc.1` become prereleases. Install them with:

```sh
mise use -g 'github:DominicWrege/devops-cli[prerelease=true]@0.3.5-rc.1'
```

If publication fails after creating a draft, inspect it before retrying. The workflow does not overwrite an existing release or its assets. Passing CI does not verify live Entra sign-in.

Published versions should stay immutable. `v0.2.0` was reissued once to ship static Linux x64 and Windows x64 only. Mise's version index can retain the deleted assets and checksums, so the installation check sets `MISE_USE_VERSIONS_HOST=false` to read GitHub directly. Run `.github/workflows/install-smoke.yml` manually to recheck a published version without rebuilding it.
