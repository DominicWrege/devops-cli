# ADR 0005: Use text-only CLI output

Status: accepted. Supersedes [ADR 0004](0004-cli-output-and-interaction.md).

## Context

The user removed JSON output after initially choosing text by default with an optional JSON flag. The CLI remains intended for agents; humans use the TUI.

## Decision

- Produce text-only CLI output, including for agent use.
- Do not implement JSON output or a `--json` flag.
- Retain the no-prompt rule for CLI commands.

This decision concerns command output, not API request and response formats.

## Consequences

- Agent examples must use text output rather than requesting JSON.
- Text formatting, errors, stdout and stderr handling, and exit codes still need definition.
- Missing input must not trigger a prompt or launch the TUI.
- Authentication bootstrap happens only in the TUI. CLI setup saves explicit configuration without signing in; CLI operations acquire silently or fail without prompting.
