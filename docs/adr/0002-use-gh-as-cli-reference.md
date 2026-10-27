# ADR 0002: Use GitHub CLI as the CLI design reference

Status: accepted.

## Context

The application provides a CLI and TUI for personal Azure DevOps use. The user chose GitHub CLI, `gh`, as the reference for the CLI.

## Decision

Use `gh` to guide CLI command design and interaction patterns. Adapt commands to Azure DevOps rather than copying GitHub terminology where the concepts differ.

This does not commit the project to feature parity, exact flag compatibility, or implementing `gh` in Rust. The TUI remains a separate design task, launched through the accepted explicit `tui` subcommand.

## Consequences

- Evaluate resource-and-action commands such as `pr list`, `pr view`, and `repo create`.
- Keep pipeline definitions distinct from individual runs when deciding command structure.
- Choose work item naming explicitly rather than assuming Azure DevOps work items are equivalent to GitHub issues.
- Decide output formats, context detection, prompts, and authentication behavior during the interview.
- The reference does not expand the agreed first-release scope.

## Reference

- [GitHub CLI manual](https://cli.github.com/manual/gh)
