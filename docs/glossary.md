# Glossary

## gh

GitHub's command-line client. This project uses its CLI design as a reference, while targeting Azure DevOps rather than GitHub.

## CLI

A command-line interface intended for agents. It accepts commands and arguments, prints text output, and exits without prompting. JSON output is out of scope. The README defines tab-separated tables, labeled details, stdout/stderr handling, and exit codes.

## TUI

An interactive terminal user interface, launched with the binary's explicit `tui` subcommand.

## Azure DevOps

Microsoft's service for repositories, pull requests, pipelines, work items, and artifacts. This is the CLI's target service, distinct from Azure infrastructure management.

## Microsoft Entra sign-in

The selected identity provider. Windows reuses an existing Entra sign-in through WAM. Linux/WSL uses OAuth 2.0 device-code login. The TUI handles required human interaction, and agents acquire tokens through the CLI without interactive prompts.

## Device-code login

The selected Linux/WSL login flow. The TUI shows a Microsoft verification URL and code; the user signs in through a browser. Subsequent token renewal uses refresh credentials without human interaction when tenant policy permits it.

## Web Account Manager

Windows' authentication broker, also called WAM. This is the selected authentication approach. It acquires tokens for accounts known to Windows and manages token renewal. Use the supplied Rust adapter reference through the `windows` crate; validate its adaptation to the Azure DevOps SDK.

## Silent token renewal

Acquiring a new access token without human interaction, using a valid refresh session. Required for this application. Revocation, expiry, or tenant policy can still require another human sign-in; the CLI must report that need without prompting.

## Pull request

A request to merge changes between branches in an Azure DevOps repository. Finding PRs that need attention is the first identified PR workflow.

## Repository

An Azure DevOps source code repository. The first release creates empty remote Git repositories and returns their URLs, without an initial commit, local clone, or local checkout.

## Pipeline

An Azure DevOps build or deployment pipeline. The first release supports YAML-defined pipelines, not classic build or release pipelines. It includes listing, inspection, triggering, watching, cancellation, rerun/retry, and registration against existing YAML. Retry granularity still needs verification.

## Pipeline run

An execution of a pipeline definition, with its own status, stages, jobs, and logs. Registering a definition and triggering a run are separate operations.

## Work item

An Azure DevOps record for work, such as a bug, task, or user story. Work items are deferred until after the first release.

## Organization

An Azure DevOps Services organization that contains projects. Initial setup must identify the organization before listing its projects.

## Project

An Azure DevOps project within an organization. TUI setup queries available projects and saves organization/project defaults in `~/.config/devops/settings.json`. Command flags override these defaults for one invocation without saving.
