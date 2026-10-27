# ADR 0008: Discover projects during setup and save a user default

Status: accepted.

## Context

The user wants initial setup to query Azure DevOps projects and allow selection of a project for subsequent use.

## Decision

During initial setup, discover organizations available to the signed-in user and let the user select one. Query projects in the chosen organization and allow the user to persist a default project in `~/.config/devops/settings.json`.

The user explicitly chose the home configuration directory. Resolve `~` to the user's home directory on each platform. On Windows this means `%USERPROFILE%\.config\devops\settings.json`, not AppData. The `devops` directory name remains provisional until the application name is settled.

Use a TUI setup wizard for interactive organization/project selection. The CLI setup equivalent accepts explicit arguments and never prompts.

Persist the selected organization and project as defaults. Normal command flags may override these defaults for one invocation only; overrides must not modify the settings file. Do not change context implicitly based on the current Git remote.

## Consequences

- Discover organizations rather than requiring the user to type an organization URL as the primary setup flow. SDK support, identity lookup, pagination, and error handling for discovery still need verification.
- Persist the organization alongside the project so later commands address the intended organization. Exact schema and identifier representation remain implementation details.
- The initial settings schema has optional `organization`, `project`, `project_id`, `tenant_id`, and `client_id` fields. TUI setup saves project name and returned ID; commands address the saved name or an explicit name/UUID override. Invalid settings fail rather than being silently overwritten.
- Preserve unrelated settings when updating the chosen project. Do not store tokens in this settings file.
- JSON configuration does not reintroduce JSON command output.
- Human selection belongs in the TUI wizard, not an interactive CLI `init` exception.
- Native Windows and WSL each use their own home settings and session. Sharing is not automatic.
