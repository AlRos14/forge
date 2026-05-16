<div align="center">

<img src="assets/forge-wordmark.png" alt="Forge" width="420">

**The local-first workflow engine for coding agents.**

[![CI](https://github.com/ForgeAILab/forge/actions/workflows/ci.yml/badge.svg)](https://github.com/ForgeAILab/forge/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/ForgeAILab/forge?include_prereleases&sort=semver)](https://github.com/ForgeAILab/forge/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Status: public beta](https://img.shields.io/badge/status-public%20beta-orange.svg)](#status)

Run Claude Code, Codex, Gemini, and other agents through a real task lifecycle —
isolated git worktrees, CI gates, review, merge — driven by a REST API, an
MCP endpoint, and a built-in web UI. No cloud, no lock-in, MIT licensed.

[Quickstart](#5-minute-quickstart) · [Why Forge](#why-forge) · [Docs](docs/) · [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md)

</div>

---

## Why Forge

Most coding agents can edit files. Forge helps you **operate them safely**.

- **Structured task lifecycle** — `todo → in_progress → review → merging → done`, with audit log and cancellation paths.
- **One isolated git worktree per task** — agents can't step on each other, and you can throw work away without polluting your repo.
- **Review gates with CI** — define `ci_steps` per task; the review runner blocks merge until they pass.
- **Local-first by default** — single binary, SQLite, binds to `127.0.0.1:8080`. No accounts, no telemetry, no SaaS.
- **BYO agent** — first-class adapters for Claude Code, Codex, Gemini, opencode, and a generic shell executor. Add your own.
- **Multiple surfaces** — REST API, MCP JSON-RPC, `forge-ctl` CLI, and a React web UI ship in the same binary.

## Who it's for

- **Solo developers** running multiple coding agents in parallel and tired of manually juggling branches and reviews.
- **Small engineering teams** piloting agent workflows who need worktree isolation, audit trails, and a review gate before code lands on `main`.
- **Builders** who want a local, hackable control plane for AI coding work — not another hosted dashboard.

If you want a chat UI bolted onto your editor, Forge is not for you. Try Cursor, Continue, or Cline instead.

## 5-minute quickstart

```bash
# Install via Homebrew (macOS / Linux)
brew install forgeailab/tap/forge

# Or grab the latest release directly
curl -fsSL https://raw.githubusercontent.com/ForgeAILab/forge/main/install.sh | bash

# Start the server with seeded demo data
forge --demo

# Open the web UI
open http://localhost:8080
```

That's it — you should see a demo project with a labelled task and a fake daemon report. From here:

- Drive a real task end-to-end → [docs/getting-started.md](docs/getting-started.md)
- Wire up Claude Code / Codex / Gemini → [docs/getting-started.md#agents](docs/getting-started.md#configuring-agents)
- Hit the API directly → [docs/api.md](docs/api.md)

Prefer to build from source? `cargo run -p forge-cli -- --demo`.

## Demo

<div align="center">

<video src="assets/demo.mp4" controls width="100%" muted autoplay loop playsinline></video>

<sub><em>30s walkthrough — task lifecycle, isolated worktrees, review gate, merge.</em></sub>

</div>

## Core concepts

| Concept | What it is |
|---|---|
| **Project** | A workspace grouping repos, tasks, agents, and a workflow definition. |
| **Repo** | A pointer to a local git checkout that tasks operate on. |
| **Task** | A unit of agent work with a state, optional CI steps, and an audit log. |
| **Agent** | A registered AI executor (Claude Code, Codex, shell, …) bound to a daemon. |
| **Daemon** | The local process that reports installed CLIs and runs executions. |
| **Worktree** | An isolated git checkout created per task, cleaned up on `done`/`cancelled`. |
| **Review gate** | The CI steps + optional human approval that block `review → merging`. |

Deeper dive → [docs/architecture.md](docs/architecture.md).

## Documentation

| Doc | What's in it |
|---|---|
| [Getting started](docs/getting-started.md) | Install, first project, agents, end-to-end task walkthrough. |
| [Architecture](docs/architecture.md) | Crate graph, task state machine, database, event bus. |
| [API reference](docs/api.md) | REST endpoints, query params, pagination, MCP tools, SSE. |
| [forge-ctl CLI](docs/cli.md) | Subcommands, daemon link, scripted runs. |
| [Execution logs](docs/execution-logs.md) | JSONL log schema and chat-history reconstruction. |
| [Changelog](CHANGELOG.md) | Per-release changes and breaking notes. |

## Status

Forge is in **public beta** (`0.1.x`). The local-first single-user product is usable
end-to-end, but APIs, schemas, and CLI flags can change without deprecation cycles.
Track breaking changes in [CHANGELOG.md](CHANGELOG.md). A stable `1.0` will land
once the workflow engine, multi-user story, and release artifacts (signing, SBOMs,
Homebrew, Windows builds) are finalized.

## Contributing

Issues, PRs, and design discussion are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md)
and check `good first issue` and `help wanted` labels. By participating you agree to
the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

Please report vulnerabilities privately per [.github/SECURITY.md](.github/SECURITY.md).

## License

[MIT](LICENSE) © Forge contributors.
