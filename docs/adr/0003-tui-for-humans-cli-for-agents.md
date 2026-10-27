# ADR 0003: Use the TUI for humans and the CLI for agents

Status: accepted.

## Context

The application must provide both a CLI and TUI. The human user is the project owner, and agents will also operate Azure DevOps through the application.

## Decision

Design the TUI for the human user and the CLI for agents. Keep the accepted single binary and explicit `tui` subcommand.

Continue using `gh` as a command-design reference, but do not assume its human-facing interaction patterns suit agent use.

## Consequences

- The CLI needs an explicit contract for output, errors, exit codes, required inputs, and write safeguards.
- The TUI needs human-oriented navigation, feedback, and confirmation flows.
- Decide which operations each interface supports rather than assuming feature parity.
- Agent hosting, authentication, and permission boundaries remain open questions.
- [ADR 0005](0005-text-only-cli-output.md) defines the current output and non-interactive behavior, superseding ADR 0004.
