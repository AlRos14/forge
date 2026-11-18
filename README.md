# Forge

Forge is an orchestrated autonomous delivery platform for Human and
harness-bound Agent work. Humans set scope and make reserved decisions; Forge
coordinates durable Tasks, assigned WorkUnits, deterministic validation, review,
repository delivery, and recovery within that authority.

## What Forge does

- Coordinates a bounded Task plan and dispatches only explicitly prepared and
  assigned WorkUnits.
- Runs deterministic validation pinned to exact inputs; Gates can require exact
  check versions and passing outcomes.
- Routes review and GitHub PR feedback through durable rework and Human decision
  records.
- Supports preauthorized Agent fallback, bounded micro-fixes, and coordinator
  replanning with inspectable receipts.
- Keeps merge as a Human decision. Repository PR actions require Human approval
  by default; automatic actions need an explicit owner/admin repository policy.

See [orchestrated delivery](docs/concepts/delivery-fluency.md),
[coordinator verification](docs/concepts/review-and-validation.md), and the
[PR feedback loop](docs/concepts/pr-feedback-loop.md) for the current product
boundaries.

## Interfaces

Use the REST API, registered MCP tools, forge-ctl, or the web UI. All surfaces
use the same durable domain services and authorization. The [API reference](docs/api.md)
lists exact REST routes, MCP tool names, and SSE event names.

## Install and run

Prebuilt archives for Linux (x86-64 and ARM64, GNU and musl) and macOS (x86-64
and ARM64), with `SHA256SUMS`, are published on [GitHub Releases](https://github.com/AlRos14/forge/releases).
The installer downloads the latest release for the current platform and
installs `forge`, `forge-ctl`, and the web UI under `/usr/local` (set `PREFIX`
to change it):

    curl -fsSL https://raw.githubusercontent.com/AlRos14/forge/main/install.sh | bash
    forge

Forge binds to loopback on an OS-selected port and prints the `management_url`
to open; the first account registered on a new data directory is the
administrator. The container image is `ghcr.io/alros14/forge`; see
[deployment](docs/deployment.md).

To build from source instead, install stable Rust, Git, Node.js 24, and pnpm 10;
building forge-cli also builds the web UI.

    git clone https://github.com/AlRos14/forge.git
    cd forge
    corepack enable
    corepack prepare pnpm@10 --activate
    cargo run --locked -p forge-cli

Follow [Getting Started](docs/getting-started.md) to create a Project, connect
an optional GitHub SCM credential, configure PR feedback policy, and run a Task.
The container runs as an unprivileged forge user and Compose binds to loopback
by default. See [deployment](docs/deployment.md) before changing network
exposure.

## Documentation

- [Architecture](docs/architecture.md)
- [Task operating model](docs/concepts/task-operating-model.md)
- [WorkUnits](docs/concepts/work-units.md)
- [Review, validation, and Gates](docs/concepts/review-and-validation.md)
- [PR feedback loop](docs/concepts/pr-feedback-loop.md)
- [Optional Project plans](docs/concepts/project-plan.md)
- [REST, MCP, and SSE](docs/api.md)
- [Getting started](docs/getting-started.md)
- [forge-ctl](docs/cli.md)
- [Deployment](docs/deployment.md)
- [Release checklist](docs/release_checklist.md)
- [Changelog](CHANGELOG.md)

## Project identity and history

The workspace source version is 0.9.1.
The current schema contains migrations through V180. Migration records,
historical specifications, earlier design documents, and ProjectRelease
snapshots remain historical evidence.

The product name remains Forge until its owner provides a replacement name.
This repository preserves its MIT license and upstream attribution.
