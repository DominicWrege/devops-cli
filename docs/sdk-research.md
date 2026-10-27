# Azure SDK feasibility

Checked: 2026-10-27.

## Findings

- The [Azure SDK for Rust](https://github.com/Azure/azure-sdk-for-rust) is under active development and publishes separate service crates, including `azure_identity`.
- Microsoft's [Azure DevOps Rust API generator and client](https://github.com/microsoft/azure-devops-rust-api) declares itself deprecated and no longer maintained or supported by Microsoft. Its notice states that no further updates, bug fixes, or security patches will be provided.
- Active development of the general Azure SDK does not establish that the deprecated DevOps client is supported.

## Proposal not accepted

An Azure SDK identity integration with a small typed REST client was proposed. The user did not accept this approach and explicitly selected `azure_devops_rust_api`, with other Azure SDK crates as needed. See [ADR 0006](adr/0006-use-azure-devops-rust-api.md).

## Implementation findings

- `azure_devops_rust_api` 0.39.1 builds with `azure_core` 1.1 and `azure_identity` 1.0. Selected resource features are `git`, `build`, `pipelines`, `core`, `accounts`, `profile`, and `policy`. See `Cargo.toml` and `Cargo.lock`.
- SDK request builders cover the selected first-release operations. The application reads raw SDK responses so generated response models do not discard timeline, configuration, or run-resource fields.
- Generated request models omit YAML path/repository on pipeline creation, and draft/expected source commit on PR updates. A per-call SDK policy fills those payload gaps and supplies sparse build PATCH bodies. Another policy removes the extra leading slash generated from a root endpoint. Mock tests caught and cover these issues. See [ADR 0011](adr/0011-sdk-request-compatibility.md).
- PR and project listing follow pagination. Pipeline listing follows continuation headers and checks each definition for YAML configuration. Pipeline run listing uses the service's recent-run list. Log retrieval uses 2,000-line pages.
- Write retries are disabled. Service calls have a 60-second deadline; interrupted writes may still have reached the server. The pipeline viewer separately polls read operations every two seconds and retains the last view on errors.
- The SDK exposes build update with `retry=true`, also documented in [Builds - Update Build](https://learn.microsoft.com/en-us/rest/api/azure/devops/build/builds/update-build?view=azure-devops-rest-7.1). Actual retry eligibility and granularity remain tenant acceptance items, not a job-level retry promise.

## Still to verify live

- Windows WAM and Linux/WSL device-code behavior using the target app registration, delegated consent, and tenant policy.
- Actual endpoint behavior, throttling, policy errors, retry eligibility, deployment-job/nested timeline coverage, and final log delivery.
- Work items remain deferred. No live write test is authorized except against explicitly approved disposable resources.
