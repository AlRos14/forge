# Security Policy

The maintained source is this repository. Releases are published on [GitHub Releases](https://github.com/AlRos14/forge/releases) by this repository's workflow for `v*` tags, starting with v0.9.0. Security fixes target the latest release and `main`.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

GitHub private vulnerability reporting is **disabled** for this repository as
of the post-V2 security audit (checked again on 2026-10-28). Do not send reports
for this fork to the upstream project's contact address or post vulnerability
details publicly.

**Pending external action:** the repository owner must open
`Settings → Code security → Private vulnerability reporting` for
`AlRos14/forge` and select **Enable**, or publish a private security contact.
Do not invent or guess a contact address. Until that setting is enabled, this
repository has no documented private intake channel; this is a repository-owner
action, not something a code change can complete.

## Scope

The following source areas are in scope:

- The `forge` server and `forge-ctl` client
- The web frontend
- SQLite persistence and numbered migrations
- Git workspace and integration operations
- REST, MCP, daemon transport, and credential handling

The following are outside this repository's control:

- Vulnerabilities in third-party dependencies; report them to the affected upstream project and notify this project when they affect Forge.
