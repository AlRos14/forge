# Decisions and Deferred Questions

**Date:** 2026-08-08  
**Scope:** Forge V2 — Autonomous Work Management

---

## Accepted decisions

## D-001 — One primary agent per task is the default

**Decision:** A capable primary agent owns planning, implementation, self-testing, diagnosis, and routine retries.

**Why:** Modern agents can sustain long tasks. Mandatory role handoffs duplicate context and add latency and cost.

**Consequence:** Planner and reviewer remain available but are conditional. The default workflow uses a worker role.

---

## D-002 — Parallelism is primarily across tasks

**Decision:** Forge optimizes for many independent tasks running concurrently rather than many agents participating in every task.

**Why:** The emerging human bottleneck is supervision and review across concurrent delegated work.

**Consequence:** Global Home, Work, review queue, capacity, dependency, and conflict handling become core product surfaces.

---

## D-003 — Worktree isolation remains mandatory but is not the headline

**Decision:** Keep one isolated workspace/worktree per task and hide routine details.

**Why:** It prevents collisions and enables durable recovery, but competing agent platforms increasingly provide similar isolation.

**Consequence:** Position Forge around reliable delegation, evidence, policy, and recovery rather than worktrees alone.

---

## D-004 — External Forge validation remains mandatory by policy

**Decision:** Agent self-validation does not satisfy required Forge checks.

**Why:** The agent may run incomplete or incorrect checks, weaken tests, or overstate success.

**Consequence:** Delivery reports clearly separate agent validation from Forge validation.

---

## D-005 — Human attention is risk- and exception-driven

**Decision:** Remove mandatory plan approval from the normal path. Request human input for ambiguity, permissions, risk, exhausted recovery, or final review according to policy.

**Why:** Human attention is the scarce resource; routine plan ceremony reduces throughput without guaranteed quality.

**Consequence:** Standard policy retains final human approval while allowing uninterrupted execution.

---

## D-006 — Independent reviewer is conditional

**Decision:** Do not dispatch a reviewer for every task.

**Why:** External deterministic checks provide stronger guarantees for many routine changes than a second LLM opinion. Semantic review is most valuable for high-risk, uncertain, or explicitly requested work.

**Consequence:** Review policy may be Never, RiskBased, or Always.

---

## D-007 — Task and run remain separate

**Decision:** A task is durable; a run is an attempt.

**Why:** Long tasks may require multiple retries, agents, or environments. Treating a run failure as task failure destroys useful continuity.

**Consequence:** UI renames execution to Run and groups attempts under the task.

---

## D-008 — Agent and runtime remain separate backend entities

**Decision:** Do not merge agent and daemon/runtime storage models.

**Why:** They answer different questions: who performs work versus where execution occurs. Merging them harms routing, portability, and multi-machine use.

**Consequence:** UI presents one agent card and hides runtime selection when automatic.

---

## D-009 — Five canonical phases are the global language

**Decision:** Backlog, Ready, Working, Review, Done.

**Why:** Project workflows may differ, but cross-project supervision needs stable semantics.

**Consequence:** Internal states map to phases. Needs attention is a derived overlay rather than a permanent sixth phase.

---

## D-010 — Keep the workflow engine; add a façade

**Decision:** Implement a new preset and intent actions over the existing workflow engine.

**Why:** The engine, hooks, review, merge, retry, and audit infrastructure are valuable and extensively tested. A rewrite adds risk without proving the UX direction.

**Consequence:** Advanced users retain custom workflows while ordinary users choose policy presets.

---

## D-011 — Unified Work is a query, not a copied board

**Decision:** Global views query the canonical task records.

**Why:** Copying tasks into a global board introduces synchronization and identity problems.

**Consequence:** Saved views are filters; tasks always belong to projects.

---

## D-012 — Delivery evidence is versioned and immutable

**Decision:** Every review submission creates a delivery report snapshot.

**Why:** Review decisions must remain attributable to the exact diff, commits, contract, and validation evidence available at the time.

**Consequence:** Request changes creates a later report rather than mutating the prior one.

---

## D-013 — Existing strict workflow remains supported

**Decision:** Preserve the current planner/coder/reviewer path as an explicit Strict preset and leave existing projects unchanged initially.

**Why:** Some users and high-risk projects need the ceremony; forced migration would be unsafe.

**Consequence:** New default changes only after compatibility and migration tooling are complete.

---

## D-014 — Typed actor is a safety dependency

**Decision:** Replace actor-control string prefixes before enabling risk-based automatic actions.

**Why:** Policy and audit cannot safely distinguish user, agent, and system while actor identity is encoded inconsistently in free-form strings.

**Consequence:** Trusted auto-merge and actor-aware hooks are blocked on the typed actor refactor.

---

## Deferred questions

## Q-001 — Should project Chat remain top-level?

**Current recommendation:** De-emphasize it and evaluate actual usage. Task Activity and structured questions should handle most execution conversation. Project Chat may remain useful for planning or exploratory work.

**Decision trigger:** usage data and user interviews after Home/Work/task-detail rollout.

---

## Q-002 — Should canonical phase be persisted on task?

**Current recommendation:** No. Derive it from workflow state to avoid drift.

**Revisit if:** global queries become too expensive and a denormalized indexed column with strict update invariants is justified.

---

## Q-003 — Should validation profiles replace existing CI/review configuration entirely?

**Current recommendation:** Wrap and adapt existing mechanisms first. Avoid duplicate command systems.

**Decision trigger:** parity analysis during F2-404.

---

## Q-004 — Should `daemon` backend routes be renamed to `runtime`?

**Current recommendation:** No in the first release. Change user-facing terminology while preserving API compatibility.

**Revisit if:** a major-version API cleanup is planned.

---

## Q-005 — Should low-risk Standard tasks require human review?

**Current recommendation:** Yes initially. Trusted policy is the explicit opt-in for eligible auto-merge.

**Decision trigger:** reliability metrics and user trust after delivery reports and validation profiles ship.

---

## Q-006 — How should non-code tasks complete?

**Current recommendation:** Delivery disposition supports approved-without-merge and accepted-artifact outcomes. The first implementation can keep code tasks primary.

**Decision trigger:** research, documentation, design, and operational task adoption.

---

## Q-007 — Should reviewer agents use a separate worktree?

**Current recommendation:** Use a read-only snapshot or non-mutating access to the worker’s exact head. Do not allow reviewer mutation by default.

**Decision trigger:** implementation constraints in the review runner and local CLI tools.

---

## Q-008 — Should task-contract acceptance criteria be normalized rows?

**Current recommendation:** JSON first, with risk and validation profile normalized for queries.

**Revisit if:** criteria require collaborative editing, per-item audit, or analytics at scale.

---

## Q-009 — Should Forge automatically split oversized tasks?

**Current recommendation:** Recommend a split and generate a preview; do not silently decompose active work in the first release.

**Decision trigger:** project-steward capability and dependency UX maturity.

---

## Q-010 — Should Forge automatically transfer exhausted work?

**Current recommendation:** Recommend transfer; require user approval unless project policy explicitly opts into safe automatic transfer between equivalent agents.

**Decision trigger:** transfer reliability and context-portability metrics.

---

## Q-011 — How should agent recommendations be ranked?

**Current recommendation:** eligibility first, then project access, capability, availability, recent reliability, and estimated cost. Always explain the recommendation.

**Do not:** create a hidden global quality score without inspectable evidence.

---

## Q-012 — Should Home replace Operations entirely?

**Decision:** No. Home is the user-facing work surface; Operator Diagnostics preserves system-level execution details.

---

## Explicitly rejected alternatives

### R-001 — Rewrite Forge as a full Jira clone

Rejected because it expands scope into organizational planning instead of agent execution supervision.

### R-002 — Delete the workflow engine and hardcode five states

Rejected because custom workflows, hooks, strict policy, recovery, and integrations are valuable. The problem is exposure, not necessarily engine capability.

### R-003 — Merge agent and daemon tables

Rejected because it prevents portable agents, automatic routing, and multi-runtime capacity management.

### R-004 — Trust agent-reported tests and remove hard validation

Rejected because self-validation is not independent evidence.

### R-005 — Keep planner/coder/reviewer for every task but hide it

Rejected as the default because it still adds cost, latency, duplicated context, and unnecessary failure points even if visually hidden.

### R-006 — Treat every run failure as a task failure state

Rejected because it blocks durable recovery and creates noisy board movement.

### R-007 — Add Needs Attention as a mandatory board column

Rejected because attention is orthogonal to phase. A task can be Working and need a question answered or be Review and need approval.

