# ADR 0006: Use azure_devops_rust_api

Status: accepted.

## Context

The user requires Rust Azure SDK crates and explicitly confirmed `azure_devops_rust_api` for Azure DevOps operations. Microsoft's repository marks the client deprecated and unsupported. This status was presented before the user confirmed the choice.

## Decision

Use `azure_devops_rust_api` for Azure DevOps Services operations, with other Azure SDK crates as needed. Do not replace it with a handwritten REST client without a new decision.

## Consequences

- Accept the risk of an unsupported upstream client with no promised bug fixes or security patches.
- Verify that the selected version covers the required PR, pipeline, and repository operations. Work items were subsequently deferred until after the first release.
- Check compatibility with supporting Azure SDK crates rather than assuming their latest versions work together.
- Choose versions and feature flags before implementation.
- If required operations are missing or incompatible, bring the limitation back for a decision rather than silently changing the approach.

## Reference

- [Microsoft Azure DevOps Rust API repository](https://github.com/microsoft/azure-devops-rust-api)
