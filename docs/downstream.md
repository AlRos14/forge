# Downstream relationship

This repository is the operational downstream maintained at
[`AlRos14/forge`](https://github.com/AlRos14/forge). It derives from
[`ForgeAILab/forge`](https://github.com/ForgeAILab/forge), which remains the
upstream source for useful fixes and features.

Downstream work does not wait for upstream review. Generic, well-contained
changes may still be proposed upstream when doing so is inexpensive, while
changes to authority, evidence retention, orchestration, project knowledge,
or agent-role semantics are developed here first.

## Current divergence

The operational line first diverged from upstream commit `d49fac7` on
2026-08-31.

| Area                    | Downstream behavior                                                                                                                                                             | Upstream status                                                                                           |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Task action bodies      | Empty, omitted, whitespace-only, and JSON `null` bodies are accepted; the web client omits the JSON content type when no body is sent.                                          | Generic upstream candidate ([PR #38](https://github.com/ForgeAILab/forge/pull/38), closed before review). |
| Harness model discovery | Codex and Cursor discover their installed model catalogs with bounded fallbacks; custom model ids remain valid and reasoning controls are only shown when explicitly supported. | Generic upstream candidate ([PR #39](https://github.com/ForgeAILab/forge/pull/39), closed before review). |

Architectural downstream changes will be added to this table when they land.
The rationale and experiments that drive those changes live in the separate
`AlRos14/agentic-engineering` repository.

## Integrating upstream

The local checkout keeps two remotes:

```text
origin    https://github.com/AlRos14/forge.git
upstream  https://github.com/ForgeAILab/forge.git
```

Before integrating upstream, inspect the commits and affected public surfaces.
Merge or rebase only after reconciling intentional entries in the divergence
table, then run the complete Rust and frontend validation appropriate to the
combined change. Upstream integration is an economic choice, not a release
requirement.
