# Authentication crate research

Checked: 2026-10-27. Initial adapters are implemented and dependencies build locally. Cache/refresh behavior has mock coverage. Native Windows WAM, WSL device-code login, and target tenant integration still require live acceptance.

## Required behavior

- Built-in Microsoft Entra sign-in initiated through the TUI.
- Reuse the existing Windows Entra account, as clarified by the user. Permit required human authentication interaction only through the TUI.
- CLI agents reuse the sign-in without prompting.
- Silent token renewal when the session and tenant policy permit it.
- Continue using `azure_devops_rust_api` for DevOps operations.
- Do not require Azure CLI, `az login`, or a Python runtime. The user explicitly rejected this route.

## Selected approach and researched alternatives

### User-provided working reference

The user supplied [SNcom-GmbH/az-mise-plugin](https://github.com/SNcom-GmbH/az-mise-plugin) and reports that it works. Reviewed commit: `60959ab1b7ff637265d13168909a34655600f6fb`.

- Its [Cargo.toml](https://github.com/SNcom-GmbH/az-mise-plugin/blob/60959ab1b7ff637265d13168909a34655600f6fb/Cargo.toml) uses the exact `azure_core` and `azure_identity` declarations supplied by the user, plus Windows-only `windows` and `windows-future` dependencies.
- Its [WAM adapter](https://github.com/SNcom-GmbH/az-mise-plugin/blob/60959ab1b7ff637265d13168909a34655600f6fb/src/auth/wam.rs) implements `azure_core::credentials::TokenCredential` directly. WAM support comes from this adapter, not a built-in `azure_identity` WAM credential.
- It acquires silently first, permits native interactive broker UI only when explicitly allowed, and silently reacquires when the cached access token has less than five minutes remaining.
- WAM owns the underlying session and refresh credentials. The adapter keeps an access token in memory and persists only the selected Windows account ID, not access or refresh tokens.
- It uses COM/WinRT desktop interop on a blocking thread, including a parent window for interactive broker requests.
- It requires no Azure CLI executable, Python runtime, MSAL DLL, browser launcher, or device-code flow.

The user explicitly confirmed WAM. Use this as the authentication design reference. Adapt its Storage resource to Azure DevOps and supply the resulting credential to `azure_devops_rust_api`. Keep CLI acquisition non-interactive even where the reference permits interactive first use; only an explicit human action in the TUI may permit broker UI.

The reference defaults to the Azure CLI public-client app ID. Reusing that ID does not require installing Azure CLI, but application identity and tenant permission requirements remain separate decisions. Do not adopt that default silently.

The reference's workplace-test document still lists corporate-tenant validation items. Distinguish the user's report of a working reference from validation of this application's Azure DevOps adaptation. The implementation now adapts the WAM module with attribution; live validation remains pending.

### Windows SSO through Web Account Manager

The user clarified that Windows is already signed in with Entra. [Web Account Manager](https://learn.microsoft.com/en-us/entra/msal/dotnet/acquiring-tokens/desktop-mobile/wam) is the Windows authentication broker for accounts known to the OS and supports silent token acquisition and broker-managed token maintenance. An OS login does not itself grant this application an Azure DevOps access token; app registration, consent, requested resource, and tenant policy still apply.

Microsoft's Rust [`windows` crate bindings](https://microsoft.github.io/windows-docs-rs/doc/windows/Security/Authentication/Web/Core/struct.WebAuthenticationCoreManager.html) expose `WebAuthenticationCoreManager`, including `GetTokenSilentlyAsync` and interactive request methods. This is a Rust-native integration candidate without Azure CLI or Python. It is not a verified high-level Entra authentication library.

Design baseline from the supplied reference: Windows WAM through `windows`, a small `azure_core::credentials::TokenCredential` adapter, and the selected DevOps SDK. `azure_identity` remains part of the requested SDK dependency set, but its current public credentials do not themselves expose WAM or automatically reuse the Windows Entra session.

Validate desktop/unpackaged-app interop, account selection, public-client registration, Azure DevOps resource requests, window ownership for TUI-initiated interaction, broker result handling, and SDK integration on Windows before accepting the design. Do not substitute MSAL.NET examples as proof that the Rust integration works.

Windows session context matters. Microsoft warns that broker acquisition requires the appropriate active user session; Windows services, impersonation, WSL, remote machines, and CI must not be assumed to share it. Agent execution context is still an open question. CLI authentication must never fall back to a UI request automatically.

The user requires native Windows agents and agents in WSL on the same PC. Native Windows uses WAM. The user chose a native Linux OAuth device-code flow rather than Windows-executable invocation or a bridge. Remote hosts and CI are not first-release requirements.

The user selected Microsoft Entra OAuth 2.0 device-code authentication for Linux/WSL through `oauth2`, exposed to the DevOps SDK through a `TokenCredential` adapter. See [ADR 0009](adr/0009-linux-device-code-authentication.md). This does not replace the Windows WAM decision or the selected DevOps SDK. Cache the session in `~/.config/devops/auth.json` with owner-only permissions, atomic writes, and a refresh file lock. App registration still needs verification.

Device-code login avoids a local callback listener and suits WSL. Tenant policy must permit the flow. Use a suitable client ID and consent, expiry-aware renewal, refresh-token rotation, and non-interactive CLI failure when another human sign-in is required. Browser authorization code with PKCE was considered but not chosen.

### oauth2

[oauth2 5.0.0](https://docs.rs/oauth2/5.0.0/oauth2/) supports authorization code with PKCE, device authorization, and refresh-token exchange through `exchange_refresh_token`.

It provides the protocol operations, not a complete Entra session manager. The application would still coordinate token expiry, renewal, persistence, refresh-token rotation, and cross-process cache access. This is a candidate for a small authentication layer, not a claim of automatic renewal out of the box.

### graph-oauth

[graph-oauth 3.0.0](https://docs.rs/crate/graph-oauth/3.0.0) documents Microsoft identity public-client support, device-code flows, and automatic token refresh. Its README states that it can be used independently of the Graph SDK.

The documented automatic-refresh examples integrate with its Graph client. Refresh behavior outside that client, public-client flow details, persistent caching, and adaptation to the DevOps SDK still require verification. The 3.0.0 docs.rs build failed; its README is available, but this is a validation risk rather than proof that the crate cannot compile. Embedded-webview authentication is not the proposed terminal-app flow.

### azure_identity

[azure_identity 1.0.0](https://docs.rs/azure_identity/1.0.0/azure_identity/) lists developer-tool credentials, managed identities, and service-principal credentials. Its published credential list does not provide a built-in browser or device-code public-client credential for this requirement. Do not assume the crate alone implements the selected TUI login and persistent user-session renewal.

The user prefers this crate. The crate itself is Rust and does not inherently require Python. Its [`AzureCliCredential`](https://docs.rs/azure_identity/1.0.0/azure_identity/struct.AzureCliCredential.html) specifically requests tokens from Azure CLI on every call and does not itself cache them. The user explicitly rejected this route because they do not want Azure CLI and its Python runtime. Do not conflate the entire crate with that credential or propose Azure CLI session reuse as the current plan.

The legacy [0.21.0 documentation](https://docs.rs/azure_identity/0.21.0/azure_identity/) exposes authorization-code, device-code, and refresh-token utility modules. It explicitly labels that version unofficial and states that Microsoft has no plans to update it. Those APIs must not be presented as capabilities of current 1.0.0. Do not silently downgrade or assume compatibility with the selected DevOps SDK.

### msal-rs

The [msal-rs README](https://github.com/freedit-dev/msal-rs) points to a daemon-app example. The crate registry lists version 0.1.2, last updated in August 2024. Its name alone does not establish Microsoft support or the required desktop public-client and persistent-cache behavior. Not a verified fit.

## Storage candidate

The current [keyring](https://docs.rs/keyring/4.2.0/keyring/) documentation recommends `keyring-core` plus selected native credential-store crates for applications that control their storage backends. For macOS, [apple-native-keyring-store](https://docs.rs/apple-native-keyring-store/1.0.2/apple_native_keyring_store/) provides Keychain access.

This was earlier cross-platform research, not a selected storage design. Current requirements are native Windows WAM and Linux/WSL OAuth; macOS Keychain is not a first-release requirement. Verify that unavailable storage fails without triggering an agent-facing prompt. Secure storage does not replace refresh coordination.

### Linux/WSL: no extra keyring service

The user rejected requiring a Secret Service-compatible daemon. Do not make GNOME Keyring, KWallet, or a similar service a mandatory Linux/WSL dependency.

The Rust [linux-keyutils-keyring-store 1.0.0](https://docs.rs/linux-keyutils-keyring-store/1.0.0/linux_keyutils_keyring_store/) crate provides a daemon-free Linux kernel keyring backend through `keyring-core`. Its documentation describes per-user persistent/session keyrings that can share credentials between processes, but all storage is in memory and is lost on kernel restart. The kernel's term "persistent keyring" does not mean disk persistence.

This may satisfy silent renewal between CLI invocations without an extra service if the user accepts signing in again after a Linux reboot or WSL shutdown. Keyring expiry or eviction can also require another TUI login. Validate actual kernel support, permissions, cross-process access, concurrent refresh coordination, and the user's WSL environment before selecting the backend. Do not assume the module's claim of general Linux availability covers every WSL version or restricted kernel.

The user subsequently requested home-directory persistence and asked to avoid extra complexity. The design baseline is a simple unencrypted cache at `~/.config/devops/auth.json`, separate from `settings.json`, with directory mode `0700` and file mode `0600`.

Use atomic replacement and a file lock for inter-process refresh coordination. Never log tokens or put them in ordinary settings. These permissions do not encrypt the credentials or protect them from arbitrary processes running as the same user or root.

Encrypted vaults, keyring services, kernel-keyring storage, and background session services are not part of the selected simple Linux/WSL design.

## SDK integration finding

The DevOps repository's [current authentication source](https://github.com/microsoft/azure-devops-rust-api/blob/main/azure_devops_rust_api/src/auth.rs) accepts an `azure_core::credentials::TokenCredential` through `Credential::from_token_credential`. This provides an integration point for a crate-backed public-client credential adapter. Check the actual selected published version before implementation; repository main is not a dependency pin.

## Earlier alternatives

Before the Windows SSO clarification, `oauth2` with a native credential store and a small SDK credential adapter was recommended but not accepted for Windows. The user-provided native WAM adapter supplies the Windows design reference. The user subsequently selected `oauth2` device-code authentication for Linux/WSL, which does require application-managed session coordination and secure refresh-token persistence.

Keychain/keyring storage is not required for the Windows WAM reference pattern. A native Linux OAuth session would need its own secure refresh-token storage; select an appropriate Linux/WSL backend separately.

## Prerequisites and validation

- A suitable Entra app registration and client ID are still required. A crate does not remove tenant registration or consent requirements.
- Request Azure DevOps tokens, not Microsoft Graph tokens. Microsoft's [Entra integration guide](https://learn.microsoft.com/en-us/azure/devops/integrate/get-started/authentication/entra-oauth?view=azure-devops) identifies the Azure DevOps resource as `499b84ac-1321-427f-aa17-267ca6975798`.
- Verify the Azure DevOps resource and delegated consent configuration for broker requests. Let WAM manage its refresh credentials rather than introducing an application-owned refresh-token flow.
- Validate silent reacquisition across process restarts and simultaneous TUI and CLI operations.
- Validate that revoked sessions and interaction-required responses fail clearly in the CLI and direct the human to the TUI.
- Never print tokens or place refresh tokens in ordinary configuration files.

The native WAM adapter is the design reference alongside the requested Azure SDK crates. App identity, agent session context, and validation of Azure DevOps SDK integration remain unresolved. Azure CLI session reuse is rejected, not a pending option.

## User-supplied SDK dependency candidate

```toml
azure_core = { version = "1.1", default-features = false, features = ["reqwest", "tokio"] }
azure_identity = { version = "1", default-features = false, features = ["tokio"] }
```

The crates.io registry confirms these versions and feature names. `azure_identity` 1.0.0 depends on `azure_core` with requirement `^1.0.0`; `azure_devops_rust_api` 0.39.1 requires `azure_core` `^1`. These version ranges admit `azure_core` 1.1.0. This is metadata verification, not a successful build.

`reqwest` and `tokio` enable transport and runtime support, not Windows SSO. With default features disabled, explicitly check HTTPS/TLS configuration, such as `azure_core/reqwest_rustls`, and the unified dependency features. The DevOps crate currently enables `azure_core` default features transitively, so disabling defaults on these two declarations alone does not guarantee defaults are disabled across the dependency graph. `azure_core` 1.1.0 declares Rust 1.88 as its minimum version.

The supplied reference configures TLS through direct `reqwest` and `rustls` dependencies, including provider initialization. Its complete manifest and transport setup must be considered together; the two-line SDK snippet alone is not the full configuration.

## Implementation status

- `src/auth/wam.rs` adapts the reference to the Azure DevOps resource and the user's configured app ID. It persists only account metadata under the home configuration directory. The MIT notice is retained in `LICENSES/az-mise-plugin.txt`. The real authentication and settings modules pass Windows cross-target type checking and Clippy using a transport-free temporary validation crate. A full native Windows build and WAM runtime validation remain open.
- `src/auth/device.rs` uses `oauth2` 5 for device authorization, polling, and refresh-token exchange. The home cache is owner-only and unencrypted, with atomic replacement and a bounded-wait inter-process file lock. Mock tests cover the initial authorization request, pending polling, successful session persistence, denied-login preservation, cached reuse, identity mismatch, rotation, concurrent renewal, revocation, and sanitized errors.
- CLI token acquisition never initiates login. Only TUI setup calls the interactive entry point. All SDK clients request the Azure DevOps `.default` scope.
- `Cargo.lock` pins the resolved SDK graph. `azure_core/reqwest_rustls` and direct rustls provider initialization cover HTTPS; disabling defaults on individual dependencies does not remove upstream transitive defaults.
- The app requires explicit tenant/client UUIDs. No third-party public-client app ID is silently adopted. App registration, consent, and tenant policy remain external prerequisites.
