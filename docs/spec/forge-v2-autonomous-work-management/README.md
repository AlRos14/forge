# Forge V2 — Autonomous Work Management

**Status:** Draft for implementation planning  
**Date:** 2026-08-08  
**Product:** Forge  
**Primary repository:** `ForgeAILab/forge`

## Purpose

This package converts the proposed Forge product direction into an implementation-ready specification.

The core shift is:

> **One capable agent owns a task end to end by default. Forge supervises many tasks in parallel, enforces external verification, preserves evidence, and requests human attention only for decisions, risk, or exceptions.**

Forge remains technically powerful, but the ordinary user should only need to understand:

- **Project** — where work belongs.
- **Task** — the durable unit of work.
- **Agent** — who is responsible for the task.
- **Review** — where a verified delivery is accepted or changed.

Runs, runtimes, worktrees, workflow states, retry budgets, and recovery remain available but become supporting infrastructure rather than the primary product model.

## Decisions treated as locked for this specification

1. **One primary agent per task by default.** The primary agent plans, implements, self-tests, diagnoses, and retries within one persistent task context.
2. **Parallelism happens primarily across tasks.** Multi-agent work inside a single task is conditional on risk, specialization, exploration, or failure.
3. **Worktree isolation remains mandatory.** It is automatic and mostly invisible.
4. **Forge validation remains independent of agent self-validation.** A task cannot be considered verified solely because the agent reports success.
5. **Human gates become policy- and risk-driven.** A separate plan approval is not part of the normal path.
6. **Independent agent review is conditional.** It is enabled for high-risk changes, explicit requests, or recovery/escalation.
7. **Task and run are separate concepts.** A failed run does not make the durable task terminally failed.
8. **Agent and runtime remain separate backend entities.** The UI hides runtime selection unless it affects availability, permissions, or location.
9. **The workflow engine is retained.** Forge adds a simpler default workflow and a product façade rather than rewriting the engine first.
10. **The unified board is a query over project tasks.** Tasks are not duplicated into a separate global board.

## Package contents

| File | Purpose |
|---|---|
| `product-spec.md` | Complete product requirements, user journeys, information architecture, policies, and success metrics. |
| `technical-design.md` | Domain model, API, persistence, workflow, migration, security, and code-level implementation design. |
| `implementation-plan.md` | Phased epics and tasks with dependencies, code targets, suggested agent profiles, and acceptance criteria. |
| `acceptance-tests.md` | Product, API, workflow, migration, failure-recovery, and UI acceptance scenarios. |
| `execution-plan.yaml` | Machine-readable task graph suitable for turning into Forge tasks or GitHub issues. |
| `default-autonomous-workflow.json` | Reference configuration for the new one-primary-agent workflow preset. |
| `decisions.md` | Architectural and product decisions, tradeoffs, and questions intentionally deferred. |

## Recommended execution sequence

Do not attempt the entire product change in one task. Execute the package in this order:

1. **Introduce canonical phases and the new workflow preset.** This establishes a simpler product model without removing legacy behavior.
2. **Create the global Home, Work, and Review surfaces.** Reuse existing task diagnostics and operations data wherever possible.
3. **Simplify task creation and task detail.** Rename executions to runs in user-facing copy and make delivery evidence the center of review.
4. **Add task contracts, validation profiles, and project autonomy policies.** Start with JSON-backed schemas to preserve iteration speed.
5. **Add risk-driven gates and conditional independent review.** Preserve the existing strict workflow as an opt-in preset.
6. **Hide runtime management and complete migration tooling.** Only then deprecate planner/coder/reviewer assumptions in the default UX.

## Suggested agent allocation for the current model portfolio

These are execution roles, not permanent product roles.

| Work type | Suggested model profile |
|---|---|
| Product architecture, schema design, migration decisions | GPT-5.6 Sol or Claude Opus-class reasoning model |
| Broad backend implementation and workflow refactors | Claude Sonnet/Opus-class coding model or GPT-5.6 |
| Frontend implementation with explicit Playwright coverage | Gemini 3.6 Flash, followed by independent review |
| Repetitive migration, generated bindings, test expansion, documentation sweeps | GLM-5.2 |
| Adversarial review, failure-mode analysis, security and policy review | Claude Opus-class model or Grok 4.5 |

The default product behavior remains one agent per task. The table above describes how to distribute implementation tasks across the Forge V2 project.

## Definition of completion for the overall initiative

Forge V2 is complete when a new user can:

1. connect a repository;
2. create a task with only a title and optional description;
3. assign or accept a recommended agent;
4. let that agent plan, implement, and self-test without a mandatory plan gate;
5. see one clear progress state while work is running;
6. receive a delivery report backed by external Forge validation;
7. approve or request changes from one review surface;
8. monitor multiple projects from a unified Work view;
9. understand runtime failures without learning the daemon model; and
10. migrate an existing strict-workflow project without losing task history, runs, workspaces, or audit evidence.

