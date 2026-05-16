# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.x     | :white_check_mark: |

Forge is in pre-stable development. Security fixes are applied to the latest commit on `main`.

## Reporting a Vulnerability

**Please do not open a public issue for security vulnerabilities.**

To report a vulnerability, use [GitHub's private vulnerability reporting](https://github.com/ForgeAILab/forge/security/advisories/new) or email [mai@takario.com](mailto:mai@takario.com).

You can expect:
- Acknowledgment within 48 hours
- An initial assessment within 7 days
- A fix or mitigation plan within 30 days for confirmed vulnerabilities

If you have not received a response within 48 hours, please follow up via email.

## Scope

The following are in scope:
- The Forge server binary (`forge-cli`)
- The client binary (`forge-ctl`)
- The web frontend
- SQLite database handling
- Git worktree operations
- MCP endpoint

The following are out of scope:
- Issues in third-party dependencies (report upstream, but feel free to notify us)
- Denial of service against the local-only server (it binds to 127.0.0.1 by default)
