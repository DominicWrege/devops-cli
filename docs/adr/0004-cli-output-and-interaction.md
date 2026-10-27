# ADR 0004: Default to text with explicit JSON output and no CLI prompts

Status: superseded by [ADR 0005](0005-text-only-cli-output.md). The user subsequently removed JSON output from scope. The decision below records the earlier choice, not the current requirement.

## Context

Agents use the CLI, while the human user uses the TUI. GitHub CLI is the command-design reference. The user chose readable text by default with an explicit JSON option rather than JSON by default.

## Decision

- CLI commands produce readable text by default.
- Agents request machine-readable output with `--json`.
- CLI commands never prompt for input.

## Consequences

- Agent instructions and examples must explicitly request JSON.
- Missing input must not trigger a prompt or launch the TUI.
- JSON schema, error format, stdout and stderr handling, and exit codes remain undecided.
- Whether `--json` accepts a field selection, as in some `gh` commands, remains undecided.
- Authentication bootstrap must respect the no-prompt CLI rule; its mechanism remains undecided.
