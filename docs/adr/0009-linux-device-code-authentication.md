# ADR 0009: Use device-code authentication on Linux and WSL

Status: accepted.

## Context

Native Windows uses WAM. The user wants a separate Linux/WSL authentication approach without Azure CLI or Python and selected Microsoft Entra OAuth 2.0 device-code login over browser authorization code with PKCE.

## Decision

- Use the Rust `oauth2` crate for Entra public-client device authorization and refresh-token exchange on Linux/WSL.
- The TUI displays the verification URL and code, then reports login progress. The user completes Microsoft sign-in in a browser.
- Expose the credential to `azure_devops_rust_api` through an `azure_core::credentials::TokenCredential` adapter.
- Acquire and renew tokens silently after setup when the session permits it. The CLI must not initiate a device-code challenge or any other interactive login.
- Keep Windows authentication on WAM; do not require a Windows binary or broker bridge for Linux/WSL.
- Do not require an extra Linux Secret Service/keyring daemon. The user rejected that dependency.
- Persist the Linux/WSL authentication session in `~/.config/devops/auth.json`, separately from ordinary settings. Following the user's request for home-directory storage without extra complexity, use a simple unencrypted, owner-only cache with directory mode `0700` and file mode `0600`.
- Use atomic cache replacement and a file lock around refresh/read-update operations. Do not add an encrypted vault, background authentication service, kernel-keyring dependency, or Windows bridge.

## Consequences

- No local browser callback listener is required.
- A suitable public-client registration, Azure DevOps resource access, delegated consent, and tenant permission for device-code authentication are prerequisites.
- Request `offline_access` for refresh-token use. `oauth2` supplies protocol exchanges, not automatic persistent session management; the credential layer must coordinate expiry, renewal, and refresh-token rotation.
- Never store access or refresh tokens in `~/.config/devops/settings.json` or logs. The separate cache is unencrypted at rest; file permissions do not protect tokens from root or arbitrary processes running as the same user.
- Kernel-only storage was considered, but the user requested home-directory persistence instead.
- Coordinate independent CLI processes and a running TUI through the cache file lock rather than a separate service.
- Tenant policy or session revocation can require another TUI login. Do not promise indefinite silent renewal or silently switch authentication methods.
- Windows and Linux/WSL settings and sessions are not automatically shared.
