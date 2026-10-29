# Specifications index

## Current contracts

The current product and runtime contracts are **not** maintained in this
directory. Use:

- [Architecture](../architecture.md)
- [Architecture V2 and invariants](../migration/architecture-v2.md)
- [Domain concepts](../concepts/)
- [REST, MCP, and SSE](../api.md)
- [CLI](../cli.md)
- [ADR index](../adr/)

## Historical specification packages

Everything under `docs/spec/` is retained as historical planning evidence. These
documents describe proposals, baselines, and retired implementations from before
or during the PR0–PR15 migration. They are not current operating instructions and
must not be used to infer live authority, routes, schemas, or UI surfaces.

| Package | Historical status | Current disposition |
|---|---|---|
| [forge-v2-autonomous-work-management](forge-v2-autonomous-work-management/README.md) | Draft for implementation (2026-08-08), never the shipped contract | Superseded by the Actor/Execution/WorkUnit/Gate architecture. Some proposals were implemented in a different form; others (for example the retained workflow engine and single-owner-agent model) were retired. |
| [add-project-agent-federation-2026-08-12](changes/add-project-agent-federation-2026-08-12/proposal.md) | Marked completed at the time | Retired by PR11 and removed from runtime/public authority by PR12–PR13. |
| [add-project-charter-milestones-2026-08-13](changes/add-project-charter-milestones-2026-08-13/proposal.md) | Proposed; never shipped as a current workflow | ProjectRelease remains a read-only historical snapshot. Charter, milestone, Genesis, and Project OS operations are retired. |
| [update-agent-workbench-and-provider-login-2026-08-14](changes/update-agent-workbench-and-provider-login-2026-08-14/proposal.md) | Partially implemented at the time | Project Agent workbench and Agent Chat surfaces are retired. Current Agent/provider/daemon settings are documented under [concepts](../concepts/) and [API](../api.md). |

Status labels in old package files (`proposed`, `implementing`, `completed`) are
historical metadata from their own authoring period. They do not describe the
current repository.
