# ADR 0007: Provide built-in Microsoft Entra sign-in

Status: accepted.

## Context

The human user operates the TUI and agents operate a non-interactive CLI. The target is Azure DevOps Services. The user selected built-in Microsoft Entra sign-in rather than reusing Azure CLI login or supplying a PAT.

## Decision

Use native Windows Web Account Manager, WAM, for Microsoft Entra authentication. The user explicitly confirmed WAM after supplying the working reference. Acquire tokens silently for the existing Windows Entra account where possible. The TUI handles any required human authentication interaction; the CLI never opens broker UI. Do not require Azure CLI login.

The user explicitly rejected an Azure CLI dependency and its Python runtime. Authentication must not require `az`, `az login`, or Python.

The user also requires silent token renewal. Renew without human interaction when the Entra session and tenant policy allow it; do not promise indefinite unattended access.

The user clarified that the human is already signed into Windows with Entra. Reuse that OS account for SSO where possible rather than requiring an unrelated login. The TUI handles consent or reauthentication when needed, while the CLI remains silent.

Use the user-provided [az-mise-plugin WAM adapter](https://github.com/SNcom-GmbH/az-mise-plugin/blob/60959ab1b7ff637265d13168909a34655600f6fb/src/auth/wam.rs) as the design reference. It uses native Windows APIs and implements the Azure SDK `TokenCredential` interface, with silent reacquisition before expiry. The user reports that the reference works. Adapt the resource/audience and SDK integration for Azure DevOps, and validate that adaptation on Windows. Do not copy the reference's Storage audience or assume its default public client ID is the chosen application identity.

## Consequences

- The application needs an appropriate public-client app registration and tenant configuration.
- Select the browser or device-code sign-in flow after checking SDK support and tenant constraints.
- Prefer broker-owned token maintenance for the Windows SSO candidate; do not assume the application should export or store broker refresh tokens. Decide persistence and concurrent CLI/TUI access after validating the integration. See [authentication research](../authentication-research.md).
- CLI operations must not prompt or launch interactive authentication. If credentials cannot be acquired silently, report the failure and the required human action.
- Verify the Azure DevOps token audience, delegated permissions, tenant consent requirements, and compatibility with the selected DevOps client.
- Agents using the human login act with that user's granted access. ADR 0010 defines high-impact safeguards. Windows persists selected-account metadata only; WAM owns credentials. ADR 0009 defines the separate Linux/WSL cache.
