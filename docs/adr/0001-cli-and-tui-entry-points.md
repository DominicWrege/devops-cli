# ADR 0001: Use one binary with an explicit TUI subcommand

Status: accepted.

## Context

The Rust application must provide both CLI and TUI interfaces for Azure DevOps. Users need a clear way to choose the interface.

## Decision

Ship one binary. Launch the TUI through an explicit `tui` subcommand. Other subcommands run CLI operations without launching the TUI.

For illustration, `devops pr list` runs a CLI command and `devops tui` launches the TUI. The binary name and PR command syntax are not final.

## Consequences

- Users install one binary for both interfaces.
- CLI commands do not silently switch to the TUI.
- The implementation prints command usage and exits with code 2 when no subcommand is supplied. It never launches the TUI implicitly.
- Separate binaries and a default TUI launch were considered but not chosen.
