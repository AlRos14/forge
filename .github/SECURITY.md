# Security Policy

This repository has no verified published release artifacts as of the PR14 documentation audit. For source issues, the maintained source is the current `main` branch.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

GitHub private vulnerability reporting is disabled for this repository as of
the PR14 audit on 2026-10-16. Do not send reports for this fork to the
upstream project's contact address or post vulnerability details publicly. The
repository owner must enable private vulnerability reporting or publish a
private contact method before this repository can accept reports through a
documented channel.

## Scope

The following source areas are in scope:

- The `forge` server and `forge-ctl` client
- The web frontend
- SQLite persistence and numbered migrations
- Git workspace and integration operations
- REST, MCP, daemon transport, and credential handling

The following are outside this repository's control:

- Vulnerabilities in third-party dependencies; report them to the affected upstream project and notify this project when they affect Forge.
