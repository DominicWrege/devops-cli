# Pipeline TUI

Status: initial implementation and model/render tests complete; live tenant usability and timeline coverage pending.

## Confirmed requirements

- Observe a YAML pipeline run's progress in the TUI while it executes.
- Show tasks in a left sidebar, with their execution status.
- Use the main pane for task logs, following the general layout of Azure DevOps' web UI.
- Follow execution and live log output within the selected job until the user selects another task or scrolls back.
- Manual task selection or scrolling back pauses automatic following. Preserve the selected task and reading position while paused.
- Press `f` to resume following within the selected job.
- The pipeline TUI is observation-only. Triggering, rerunning/retrying, canceling, and registering pipelines stay in the CLI; do not add these write controls to the pipeline screen.

## Implemented defaults

- Group tasks by stage and job so parallel jobs and repeated task names are distinguishable.
- Show run identity, branch, overall status, and elapsed time in a header.
- Distinguish pending, running, succeeded, failed, skipped, and canceled tasks without relying on color alone.
- Keep the display usable when logs are temporarily unavailable or a connection fails.
- On wide terminals, recent runs and the task tree share the left column; logs fill
  the right column. Narrow terminals place logs below the two lists. `w` / `F6`
  cycles pane focus, arrows navigate, and Shift+Left/Right pans long lines.
- The run viewer retains the originating list. Select another run in its runs pane
  with Enter, or press Esc to return to that list. Tab still switches sections.

- Use Up/Down to select a stage, job, or task. Selecting a job then pressing `f` follows tasks within that job. PageUp/PageDown scrolls logs and pauses following.
- Poll status, timeline, and incremental log pages every two seconds. No push-streaming dependency.
- Fetch at most 2,000 log lines at a time. Retain at most 10,000 lines or 2 MiB for the selected task; truncate individual lines over 64 KiB. Show the first retained line offset in the title.
- Preserve absolute line offsets while paused. If retention evicts the reading position, clamp it to the first retained line instead of pretending the evicted text is still available.
- Track task identity, attempt, and log ID so stale responses cannot contaminate another task or retry attempt.
- Continue showing updated run progress when a task log is temporarily unavailable. A queued run without a timeline has an empty sidebar until records arrive.

## Deferred refinements

- Aggregated/job-wide logs, search, and filtering are not implemented. The initial viewer shows one task's logs.
- Validate sidebar and form usability at the user's terminal sizes before changing controls or retention defaults.

## API feasibility to verify

- SDK builders are implemented for run status, current build timeline, and ranged log retrieval. Local tests cover ranges, overlap rejection, attempt resets, and bounded retention.
- Verify actual line boundaries, task completion, deployment jobs, nested timelines, and final log delivery against the target tenant.
- Terminal state uses an RAII guard; async tasks keep input responsive. Local tests render the sidebar/main-pane layout without requiring a real terminal.
- Remote control characters are stripped before display. The application never adds access/refresh tokens to diagnostics; server-provided pipeline logs still need the pipeline's own secret-masking discipline.
