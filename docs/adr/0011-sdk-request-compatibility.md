# ADR 0011: Repair generated payload gaps through SDK policies

Status: implemented; tenant validation pending.

## Context

The user selected `azure_devops_rust_api`, despite its upstream deprecation, and rejected substituting a handwritten REST client. Published version 0.39.1 covers the selected endpoints but omits required request fields in a few generated models. Its root endpoint formatting also produces a doubled leading path slash.

## Decision

- Keep SDK operation builders, URLs, API versions, authentication, and HTTP transport.
- Use the SDK's supported per-call policy extension for missing YAML-registration path/repository fields, PR draft edits and merge source-head protection, and sparse build status updates.
- Normalize the doubled leading slash in a separate per-call policy. Do not derive target organizations or projects from response URLs.
- For the organization-scoped authenticated-user lookup, use a dedicated per-call
  policy to route an SDK GET to `/_apis/connectionData`. Version 0.39.1 has no
  builder for this endpoint. The policy is installed only on that call, which
  reads the raw response and retains SDK credentials, transport, and error handling.
  PR votes and assigned reviews use this lookup because the global Profiles API
  requires Entra authentication and can reject an otherwise valid PAT.
- Read raw SDK responses instead of discarding fields through incomplete generated response models.
- Verify outgoing payloads, authorization audience, path encoding, and no automatic write retries using a local mock server.

## Consequences

- No vendored SDK fork or parallel REST client is needed.
- Compatibility code stays in `src/service.rs`. The authenticated-user lookup is
  the single route override; PR, pipeline, and repository resource requests retain
  their generated SDK routes.
- Merge retains `lastMergeSourceCommit`, explicitly disables policy bypass, and does not delete branches or transition work items.
- Pipeline registration preserves the selected YAML path and repository. Build PATCH requests do not resend unrelated read-only fields.
- These policies are pinned to the current SDK behavior and need review when upgrading. Mock success does not establish actual tenant support.
