# ADR 0012: Support personal access tokens without app registration

Status: accepted.

## Context

The user cannot create Entra app registrations and explicitly requested personal access token (PAT) support, with an interactive token prompt as the default for now. Repository autodetection is the next feature, not part of this change. The existing WAM/device-code flows require a public-client application ID. This extends the Entra-only decision in ADR 0007 without adding an Azure CLI dependency.

## Decision

- `devops login` defaults to a hidden PAT prompt. The TUI has a masked PAT field. Never accept token arguments or display tokens. Retain Entra as an explicit `login --entra` / `--client-id` option.
- Verify organization access by listing at most one project before saving a CLI-entered token. TUI setup validates project listing before saving its token. Project and Team read permission is required. Save organization/project defaults only separately from secrets; `init` remains offline and non-interactive.
- Persist entered PATs per organization in `pat-ORGANIZATION.json`. Windows uses current-user DPAPI encryption without interactive UI. Linux/WSL uses the existing atomic file store with directory mode `0700`, file mode `0600`, and symlink/permission checks. No keyring daemon or Windows/WSL credential bridge is added.
- Keep `DEVOPS_PAT` as an optional, non-persisted override, taking precedence over saved credentials. Empty or malformed overrides fail explicitly. Commands other than login/TUI never prompt; a missing cached token fails with an actionable message.
- Use the Azure DevOps SDK's native PAT credential, which sends HTTP Basic authentication, not an OAuth bearer token or a fake expiring `AccessToken`.
- Skip tenant discovery, app-ID validation, WAM and device-code acquisition in PAT mode. TUI setup takes an organization and lists its projects, avoiding profile/organization discovery that an organization-scoped PAT may not permit.
- Preserve existing non-interactive CLI behavior, write safeguards, and stable authentication/authorization exit codes. Expired/revoked PATs require manual replacement; there is no silent PAT renewal.

## Consequences

- Users need no app-registration permissions, but their organization must allow PAT issuance and the token must have the appropriate resource scopes.
- Saved tokens remove the need to set an environment variable for daily use. Linux cache files remain unencrypted; Windows encrypted files are tied to the current Windows user's DPAPI keys. Treat both as sensitive.
- New settings default to PAT. Settings predating the authentication-mode field retain Entra for normal operations. Login defaults to PAT unless Entra is explicitly selected. Unset `DEVOPS_PAT` before selecting Entra.
- Automated tests cover Basic authentication, hidden/masked input behavior, token-cache round trips, redaction, old-settings compatibility, setup without an app ID, and HTTP 401/403 handling. Live tenant validation remains separate.
