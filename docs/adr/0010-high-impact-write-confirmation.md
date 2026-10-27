# ADR 0010: Confirm high-impact writes only

Status: accepted.

## Context

The agent CLI must never prompt. The PR TUI supports all selected PR actions, while the pipeline TUI is observation-only. The user chose confirmation for high-impact actions rather than every write or no confirmation.

## Decision

- Treat PR completion/merge, PR abandonment, and pipeline cancellation as high-impact actions.
- Require explicit human confirmation in the TUI for high-impact actions exposed there.
- Require an explicit acknowledgement flag for high-impact CLI commands. Never prompt or open the TUI to obtain it.
- Routine creates, edits, comments, and review votes need no extra confirmation.
- Pipeline triggering, rerunning, and retrying need no extra acknowledgement beyond the explicit CLI command. The user chose this after being informed that a YAML pipeline can deploy to production.

## Consequences

- Confirm PR merge and abandonment in the TUI. Pipeline cancellation stays in the CLI, consistent with the observation-only pipeline TUI.
- Implementation uses `--yes` and exit code 2 when acknowledgement is missing. The CLI does not authenticate or send a request first.
- Do not silently add extra acknowledgement requirements to pipeline trigger, rerun, or retry commands or attempt to classify their risk by inspecting arbitrary YAML.
- Confirmation and acknowledgement flags reduce accidental requests; they are not an authorization boundary against an agent that can supply the flag.
- Azure DevOps permissions and policies remain authoritative. Confirmation does not grant additional access or policy bypass.
