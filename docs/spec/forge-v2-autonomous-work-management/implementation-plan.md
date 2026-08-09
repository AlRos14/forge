# Implementation Plan: Forge V2 — Autonomous Work Management

**Status:** Execution candidate  
**Date:** 2026-08-08  
**Planning unit:** One task should normally produce one reviewable pull request.  
**Sizing:** `S` focused change, `M` multi-module change, `L` architectural change requiring staged review.

---

## 1. Execution rules

1. Do not combine the entire initiative into one agent run.
2. Keep the existing strict workflow operational throughout the migration.
3. Merge foundation and API changes before depending frontend work begins.
4. Every backend task must include unit or integration coverage.
5. Every user-visible flow must include Playwright coverage before its milestone closes.
6. Generated API bindings must be updated in the same pull request as source API type changes.
7. Use one primary implementation agent per task. Use a separate reviewer only for `high` risk or designated architecture tasks.
8. Do not enable risk-based auto-actions until typed actor attribution and audit reasons are complete.
9. Prefer additive fields and façade endpoints before removing or renaming existing backend concepts.
10. Treat documentation and migration tooling as product deliverables, not post-release cleanup.

---

## 2. Workstream overview

| Milestone | Goal | Exit condition |
|---|---|---|
| **M0 — Baseline and decisions** | Protect current behavior and establish architectural contracts. | Current tests are green; key decisions and fixtures exist. |
| **M1 — Workflow foundation** | Add canonical phases and the autonomous preset. | A project can run one worker agent through verified human review. |
| **M2 — Global work surfaces** | Build Home, Work, and review-oriented navigation. | User can supervise all active tasks without opening Operations. |
| **M3 — Task and review experience** | Simplify creation, task detail, runs, checks, and delivery evidence. | Review can be completed from one task-centered surface. |
| **M4 — Contracts and validation** | Add task contracts, project policies, and validation profiles. | Forge can enforce scope and required validation independently. |
| **M5 — Risk and escalation** | Add risk-based gates, independent review, recovery, and transfer. | Multi-agent behavior is exception-driven and auditable. |
| **M6 — Agent/runtime simplification** | Present agents as teammates and runtimes as hidden compute. | Normal users do not need daemon management. |
| **M7 — Migration and release** | Migrate safely, reposition product, and close compatibility gaps. | Autonomous is safe as the new-project default. |

---

# M0 — Baseline and decisions

## F2-000 — Create Forge V2 spec location and change index

- **Size:** S
- **Risk:** low
- **Suggested agent:** documentation/maintenance
- **Dependencies:** none
- **Code targets:**
  - `docs/spec/forge-v2-autonomous-work-management/`
  - optional `docs/spec/README.md`
- **Deliverables:**
  - commit this specification package;
  - add a top-level index linking product, technical, implementation, and tests;
  - establish status labels: proposed, accepted, implementing, shipped.
- **Acceptance:**
  - package renders correctly on GitHub;
  - all internal links work;
  - spec owner and decision process are documented.

## F2-001 — Capture current workflow and UI compatibility fixtures

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend/test
- **Dependencies:** F2-000
- **Code targets:**
  - `crates/api/tests/`
  - `crates/services/src/workflow/engine/tests.rs`
  - `web/e2e/`
  - test fixture directory
- **Deliverables:**
  - serialized current default workflow fixture;
  - representative legacy custom workflow fixture;
  - database fixture containing backlog, planning, active, review, merge-failed, done, and cancelled tasks;
  - current main-page render and task happy-path regression tests.
- **Acceptance:**
  - fixtures reproduce current behavior;
  - tests fail on unintentional workflow schema incompatibility;
  - fixture can be reused by migration tests.

## F2-002 — Introduce typed transition actor

- **Size:** L
- **Risk:** high
- **Suggested agent:** senior backend/architecture
- **Independent review:** required
- **Dependencies:** F2-001
- **Code targets:**
  - `crates/api-types/`
  - `crates/services/src/task_service/transition.rs`
  - `crates/services/src/workflow/engine/`
  - API, MCP, dispatcher, execution, and recovery transition call sites
- **Deliverables:**
  - typed `Actor` model for User, Agent, and System;
  - separate human-readable reason/source fields;
  - exhaustive hook-audience matching;
  - corrected emitters for MCP and agent execution paths;
  - migration-compatible audit formatting.
- **Acceptance:**
  - no behavioral control uses `starts_with("user:")` or `starts_with("agent:")`;
  - agent, user, and system transitions are attributed correctly in tests;
  - current happy path remains green;
  - audit log remains human-readable.

## F2-003 — Add product terminology adapter

- **Size:** S
- **Risk:** low
- **Suggested agent:** frontend/platform
- **Dependencies:** none
- **Code targets:**
  - i18n/locales;
  - shared frontend formatters;
  - API documentation glossary.
- **Deliverables:**
  - user-facing labels: Run for execution, Runtime for daemon, Phase for canonical workflow grouping;
  - operator diagnostics may retain backend names in secondary copy;
  - terminology helper prevents inconsistent strings.
- **Acceptance:**
  - normal UI copy no longer teaches daemon terminology;
  - existing routes and API type names remain unchanged.

### M0 exit gate

- All existing CI passes.
- Typed actor design is merged or explicitly scheduled before M5 automation.
- Legacy fixtures are committed.
- Specification decisions are accepted.

---

# M1 — Workflow foundation

## F2-100 — Add `CanonicalPhase` to workflow API types

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend
- **Dependencies:** F2-001
- **Code targets:**
  - `crates/api-types/src/workflow.rs`
  - generated Rust/TypeScript bindings
  - workflow validation tests
- **Deliverables:**
  - `CanonicalPhase` enum;
  - optional `canonical_phase` field on `StateDefinition`;
  - fallback derivation for legacy definitions;
  - workflow validation requiring explicit phase for new saves.
- **Acceptance:**
  - old workflow JSON deserializes;
  - every known current state maps correctly;
  - unknown-state fallback emits a diagnostic and remains usable;
  - generated bindings are updated.

## F2-101 — Add canonical phase to task responses and queries

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend/API
- **Dependencies:** F2-100
- **Code targets:**
  - task response conversion;
  - task list filters;
  - API query types;
  - database query adapters as required.
- **Deliverables:**
  - additive `canonical_phase` field in task response;
  - task list filter by canonical phase;
  - phase calculated from project workflow and task status.
- **Acceptance:**
  - no persisted task phase can drift from workflow state;
  - legacy and custom workflows return a phase;
  - pagination and ordering behavior remain stable.

## F2-102 — Implement `autonomous_v1` workflow preset

- **Size:** L
- **Risk:** high
- **Suggested agent:** workflow/backend
- **Independent review:** required
- **Dependencies:** F2-100, F2-002 preferred
- **Code targets:**
  - new `default_autonomous_workflow.rs`;
  - workflow registry and template service;
  - prompt builders;
  - workflow integration tests.
- **Deliverables:**
  - worker role only by default;
  - states: backlog, ready, working, review, merging, merge-failed, done, cancelled;
  - canonical phase mapping;
  - no mandatory plan gate;
  - no automatic independent reviewer in Standard behavior;
  - requested changes and validation failures resume worker context.
- **Acceptance:**
  - one worker agent completes plan, implementation, and self-test;
  - hard validation runs before review;
  - human approves review in Standard mode;
  - merge and cleanup work;
  - strict workflow tests remain green.

## F2-103 — Register workflow presets and project selection metadata

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend/API
- **Dependencies:** F2-102
- **Code targets:**
  - workflow registry;
  - project settings/types;
  - project create/update routes;
  - project settings UI later consumed by F2-607.
- **Deliverables:**
  - preset IDs: autonomous_v1, strict_multi_agent_v1, custom;
  - project creation can select preset;
  - serialized workflow remains authoritative;
  - project reports whether current workflow is an unmodified preset or custom.
- **Acceptance:**
  - creating a project with each preset succeeds;
  - editing a preset workflow marks it custom without data loss;
  - existing projects are identified safely.

## F2-104 — Create façade task actions for autonomous workflow

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/service
- **Independent review:** required
- **Dependencies:** F2-102
- **Code targets:**
  - task action service;
  - API routes;
  - MCP/CLI follow-up tasks later.
- **Deliverables:**
  - start, pause, resume, submit, request changes, approve, and cancel actions;
  - action resolves actual workflow transition rather than requiring UI state names;
  - compatibility with strict/custom workflows through capability checks.
- **Acceptance:**
  - UI clients can operate without hardcoding state names;
  - invalid actions return available actions and reason;
  - raw transition endpoint remains functional.

## F2-105 — Make same-worker follow-up the autonomous default

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend/execution
- **Dependencies:** F2-102
- **Code targets:**
  - `task_service/execution/follow_up.rs`;
  - recovery context builder;
  - execution policy tests.
- **Deliverables:**
  - request changes resumes latest worker thread;
  - validation failure resumes latest worker thread;
  - merge repair attempts same worker thread when compatible;
  - fallback to new run when resume is unsupported.
- **Acceptance:**
  - task history shows separate runs/turns without losing context;
  - no new planner or reviewer is required;
  - fallback behavior is explicit and audited.

### M1 exit gate

- New project can use autonomous_v1 end to end.
- Existing strict project behavior is unchanged.
- Task API exposes canonical phase.
- UI can use intent actions instead of raw state names.

---

# M2 — Global work surfaces

## F2-200 — Build normalized attention derivation service v1

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/domain
- **Independent review:** required
- **Dependencies:** F2-101
- **Code targets:**
  - new `attention_service.rs`;
  - existing diagnostics, review, execution, runtime, and dependency services;
  - API DTOs and tests.
- **Deliverables:**
  - attention kinds and severity;
  - deterministic primary-attention selection;
  - recommended actions;
  - initial signals from existing `awaiting_human`, blocked, failed, workflow exception, active execution, reviews, dependencies, and runtime state.
- **Acceptance:**
  - table-driven tests cover every attention kind;
  - task with no actionable condition returns none;
  - simultaneous conditions resolve consistently;
  - no mutable duplicate attention truth is introduced.

## F2-201 — Add Home API

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend/API
- **Dependencies:** F2-200
- **Code targets:**
  - new home route/service;
  - operator status reuse;
  - pagination/bounds tests.
- **Deliverables:**
  - needs attention;
  - awaiting review;
  - running now;
  - recent deliveries placeholder from current task/merge data until M3;
  - collapsed system health summary.
- **Acceptance:**
  - response is bounded and fast on representative fixture;
  - sections use task titles and user-facing summaries;
  - daemon/operator detail is absent unless work is affected.

## F2-202 — Add global Work API

- **Size:** L
- **Risk:** medium
- **Suggested agent:** backend/query
- **Dependencies:** F2-101, F2-200
- **Code targets:**
  - task list/query service;
  - DB indexes if profiling requires them;
  - API route and generated bindings.
- **Deliverables:**
  - cross-project task query;
  - filters for phase, attention, project, repo, assignee, risk placeholder, updated date;
  - cursor pagination;
  - list and board-compatible grouping.
- **Acceptance:**
  - no duplicate task records;
  - filters compose correctly;
  - response provides project and repo display metadata;
  - query plan remains acceptable on large fixture.

## F2-203 — Implement Home page

- **Size:** L
- **Risk:** medium
- **Suggested agent:** frontend
- **Review:** independent UI review recommended
- **Dependencies:** F2-201
- **Code targets:**
  - `HomePage.tsx`;
  - extracted Operations components;
  - API hooks, SSE invalidation, tests.
- **Deliverables:**
  - Needs attention, Awaiting review, Running now, Recent deliveries, System health;
  - clear empty states and actions;
  - responsive behavior matching design system.
- **Acceptance:**
  - user can open the relevant task from every item;
  - normal page contains no workspace paths or daemon IDs;
  - loading, error, and empty states covered;
  - Playwright scenario updates live when a task changes.

## F2-204 — Implement global Work page

- **Size:** L
- **Risk:** medium
- **Suggested agent:** frontend
- **Dependencies:** F2-202
- **Code targets:**
  - `WorkPage.tsx`;
  - task list primitives;
  - global canonical board view;
  - filters and saved-view local persistence.
- **Deliverables:**
  - list default;
  - board alternative;
  - project, phase, attention, assignee filters;
  - built-in views: Active, Needs attention, Awaiting review, Running, Completed recently.
- **Acceptance:**
  - cross-project list and board show the same underlying tasks;
  - filters persist per user locally or through existing settings mechanism;
  - global board uses five canonical phases.

## F2-205 — Restructure navigation

- **Size:** M
- **Risk:** medium
- **Suggested agent:** frontend
- **Dependencies:** F2-203, F2-204
- **Code targets:**
  - `app-shell.tsx`;
  - router;
  - locale strings;
  - route tests.
- **Deliverables:**
  - global: Home, Work, Projects, Agents, Settings;
  - project: Overview/Board/Tasks/Deliveries/Settings as supported;
  - Daemons and Operations moved under advanced/system routes;
  - old URLs redirect or remain directly accessible.
- **Acceptance:**
  - navigation works in full, rail, and mobile modes;
  - current project context is preserved;
  - accessibility and focus behavior remain green.

## F2-206 — Preserve Operator Diagnostics

- **Size:** S
- **Risk:** low
- **Suggested agent:** frontend/maintenance
- **Dependencies:** F2-203, F2-205
- **Code targets:**
  - existing Operations page/route;
  - Settings advanced navigation.
- **Deliverables:**
  - Operations renamed Operator Diagnostics in UI;
  - existing technical detail remains available;
  - Home reuses data without deleting diagnostics.
- **Acceptance:**
  - operators can still inspect execution, agent, runtime, retry, and cleanup pressure;
  - normal users are not routed there by default.

### M2 exit gate

- Home answers what needs attention.
- Work queries tasks across projects.
- Five-phase global board functions.
- Operator diagnostics remain available but de-emphasized.

---

# M3 — Task and review experience

## F2-300 — Simplify task creation

- **Size:** M
- **Risk:** medium
- **Suggested agent:** frontend
- **Dependencies:** F2-104
- **Code targets:**
  - task create dialog;
  - project defaults API usage;
  - component and E2E tests.
- **Deliverables:**
  - basic form: title, optional description, repo, assignee;
  - automatic project policy defaults;
  - advanced disclosure for roles, model overrides, review/merge config, and future contract fields;
  - create-and-start option.
- **Acceptance:**
  - title-only task can be created and started;
  - no planner/coder/reviewer setup appears in basic mode;
  - strict workflow project still exposes required advanced assignments when necessary.

## F2-301 — Add current-run summary DTO

- **Size:** M
- **Risk:** low
- **Suggested agent:** backend/API
- **Dependencies:** F2-101
- **Code targets:**
  - execution summary types;
  - task response conversion;
  - generated bindings.
- **Deliverables:**
  - run ID, agent, model, status, activity, elapsed time, last activity, stop reason, token/cost summary;
  - user-facing status normalization.
- **Acceptance:**
  - task card/detail does not need to reconstruct current run from raw execution fields;
  - legacy tasks without runs return null safely.

## F2-302 — Restructure task detail information architecture

- **Size:** L
- **Risk:** high
- **Suggested agent:** frontend
- **Independent UI review:** required
- **Dependencies:** F2-301, F2-200
- **Code targets:**
  - `TaskDetailPage.tsx`;
  - existing task detail panels;
  - new tab components.
- **Deliverables:**
  - Overview, Activity, Changes, Checks, Runs;
  - current attention banner;
  - primary action from execution actions/intent API;
  - advanced state and runtime details in secondary areas.
- **Acceptance:**
  - five user questions are immediately answerable: objective, responsible actor, progress, changes, safety;
  - no data or control is lost from current task detail;
  - responsive and keyboard behavior covered.

## F2-303 — Rename execution UI to Run and separate diagnostics

- **Size:** M
- **Risk:** low
- **Suggested agent:** frontend/maintenance
- **Dependencies:** F2-003, F2-302
- **Code targets:**
  - execution detail page;
  - task execution panels;
  - routes/page titles;
  - docs.
- **Deliverables:**
  - user-facing Run terminology;
  - raw logs, terminal, and tool events grouped under Diagnostics;
  - stable existing execution URLs.
- **Acceptance:**
  - no backend route rename required;
  - user can still inspect every existing technical field;
  - normal task activity does not display raw log noise by default.

## F2-304 — Add delivery report persistence and API

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/domain
- **Independent review:** required
- **Dependencies:** F2-102, F2-105
- **Code targets:**
  - migration;
  - DB model/repository;
  - delivery report service;
  - Git/diff, validation, review, and PR integration;
  - API and tests.
- **Deliverables:**
  - versioned immutable delivery snapshots;
  - scope, commits, diff, agent validation, Forge checks, reviews, exceptions, disposition;
  - delivery summary on task response.
- **Acceptance:**
  - each review submission creates a new version;
  - prior report remains unchanged after requested changes;
  - report records base/head commit and exact check evidence;
  - missing required provenance blocks verified submission.

## F2-305 — Build delivery report and Checks UI

- **Size:** L
- **Risk:** high
- **Suggested agent:** frontend
- **Independent UI review:** required
- **Dependencies:** F2-304, F2-302
- **Code targets:**
  - delivery report component;
  - Changes and Checks tabs;
  - Review actions;
  - component/E2E tests.
- **Deliverables:**
  - requested versus actual scope;
  - diff and commits;
  - agent self-validation separated from Forge validation;
  - exceptions and risks;
  - approve, request changes, independent review, and merge actions.
- **Acceptance:**
  - reviewer can make a decision without reading raw run logs;
  - check result links open exact evidence;
  - incomplete or stale validation is visually distinct.

## F2-306 — Add global review queue

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend + frontend split if needed
- **Dependencies:** F2-304
- **Code targets:**
  - review queue API;
  - Work saved view or dedicated page;
  - notification integration.
- **Deliverables:**
  - review-ready tasks across projects;
  - risk, age, agent, project, check summary;
  - bulk navigation, not bulk approval initially.
- **Acceptance:**
  - only genuinely review-ready deliveries appear;
  - latest report version is shown;
  - approving/requesting changes updates queue live.

### M3 exit gate

- A task can be created simply, executed, externally checked, reviewed, changed, and approved from task-centered UI.
- Delivery evidence is versioned.
- Runs and diagnostics are clearly separated.

---

# M4 — Contracts and validation

## F2-400 — Add task contract schema and persistence

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/API
- **Independent review:** required
- **Dependencies:** F2-001
- **Code targets:**
  - API types and requests;
  - DB migration/model/repository;
  - task create/update routes;
  - generated bindings;
  - validation tests.
- **Deliverables:**
  - objective, acceptance criteria, allowed/protected paths, validation profile, risk, budget, permissions, merge override;
  - contract version included in audit/delivery report.
- **Acceptance:**
  - existing tasks default safely;
  - malformed paths or unsupported policy values are rejected;
  - contract changes during active work are audited and surfaced to agent context.

## F2-401 — Build task contract editor

- **Size:** M
- **Risk:** medium
- **Suggested agent:** frontend
- **Dependencies:** F2-400
- **Code targets:**
  - task create advanced section;
  - Overview tab;
  - contract components/tests.
- **Deliverables:**
  - acceptance criteria editor;
  - allowed/protected paths;
  - risk and validation selectors;
  - budget and merge overrides under advanced.
- **Acceptance:**
  - basic user can ignore it;
  - advanced user can see effective inherited values;
  - active-task edits warn about execution impact.

## F2-402 — Add project autonomy policy

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/policy
- **Independent review:** required
- **Dependencies:** F2-002, F2-400
- **Code targets:**
  - API types;
  - project settings serialization;
  - policy service;
  - project routes and tests.
- **Deliverables:**
  - Standard, Trusted, Strict, Custom presets;
  - planning, independent review, human review, validation, merge, retry, and transfer policy;
  - effective task policy resolution.
- **Acceptance:**
  - policy resolution is deterministic and explains reasons;
  - task overrides merge correctly;
  - lowering high/critical inferred risk requires reason;
  - strict workflow remains compatible.

## F2-403 — Add project policy UI

- **Size:** M
- **Risk:** medium
- **Suggested agent:** frontend
- **Dependencies:** F2-402
- **Code targets:**
  - Project Settings;
  - policy summary components;
  - forms/tests.
- **Deliverables:**
  - preset selection;
  - clear consequence summary;
  - advanced controls;
  - warnings for auto-merge and reduced gates.
- **Acceptance:**
  - changing preset shows exact resulting behavior;
  - dangerous changes require explicit confirmation;
  - effective policy appears on task detail.

## F2-404 — Add validation profile persistence and service façade

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/validation
- **Independent review:** required
- **Dependencies:** F2-400
- **Code targets:**
  - DB migration/model/repository;
  - validation service façade around existing CI/review execution;
  - API routes and tests.
- **Deliverables:**
  - named project profiles;
  - command, provider-check, scope, and diff-policy checks;
  - default profile resolution;
  - immutable result evidence.
- **Acceptance:**
  - existing CI steps can be represented or adapted;
  - required checks cannot be silently removed by worker output;
  - exact commit/workspace is recorded;
  - profile CRUD is authorized and validated.

## F2-405 — Build validation profile UI and onboarding suggestion

- **Size:** L
- **Risk:** medium
- **Suggested agent:** frontend + repository-detection backend split
- **Dependencies:** F2-404
- **Code targets:**
  - Settings → Validation Profiles;
  - repository inspection suggestion endpoint;
  - profile test UI;
  - E2E coverage.
- **Deliverables:**
  - profile list/editor;
  - test command safely;
  - suggested commands from repository metadata;
  - explicit confirmation before requirement activation.
- **Acceptance:**
  - user can create a profile without editing workflow JSON;
  - failed test gives actionable output;
  - suggested commands are never silently enforced.

## F2-406 — Enforce contract scope and validation on submission

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/security
- **Independent review:** required
- **Dependencies:** F2-400, F2-404, F2-304
- **Code targets:**
  - validation service;
  - task submission action;
  - delivery report;
  - recovery flow.
- **Deliverables:**
  - changed-path comparison;
  - protected-path escalation;
  - required check execution;
  - same-agent repair on failure;
  - exception mechanism with permission and reason.
- **Acceptance:**
  - out-of-scope changes cannot appear as verified without exception;
  - protected-path changes elevate effective risk;
  - validation evidence is attached to the correct delivery version.

### M4 exit gate

- Projects define policy and validation without workflow JSON.
- Tasks can carry enforceable contracts.
- Review evidence reflects contract and validation results.

---

# M5 — Risk and escalation

## F2-500 — Implement risk inference rules

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/policy/security
- **Independent review:** required
- **Dependencies:** F2-402, F2-406
- **Code targets:**
  - policy service;
  - project risk rules;
  - delivery evaluation;
  - tests.
- **Deliverables:**
  - path-pattern rules;
  - migrations/auth/payments/infrastructure/dependencies/test-removal signals;
  - size and scope signals;
  - explainable policy reasons.
- **Acceptance:**
  - test matrix covers low through critical;
  - rules can only elevate minimum risk automatically;
  - user overrides are audited.

## F2-501 — Add conditional independent review

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/workflow
- **Independent review:** required
- **Dependencies:** F2-500, F2-304
- **Code targets:**
  - policy evaluator;
  - review dispatch;
  - read-only workspace snapshot;
  - delivery report.
- **Deliverables:**
  - reviewer dispatched only when policy or user requests;
  - reviewer cannot mutate worker workspace by default;
  - findings recorded structurally;
  - rejection returns feedback to worker context.
- **Acceptance:**
  - low-risk Standard task has no reviewer run;
  - high-risk task receives independent review;
  - reviewer identity and evidence appear in delivery report.

## F2-502 — Implement risk-based human gate and low-risk auto-merge

- **Size:** L
- **Risk:** critical
- **Suggested agent:** senior backend/security
- **Independent review:** mandatory
- **Dependencies:** F2-002, F2-500, F2-501, F2-406
- **Code targets:**
  - policy service;
  - review/merge workflow actions;
  - audit and tests.
- **Deliverables:**
  - Standard human gate;
  - Trusted low-risk auto-merge;
  - no auto-merge for protected/high/critical work;
  - stale-base revalidation policy;
  - explicit policy reasons.
- **Acceptance:**
  - exhaustive policy matrix passes;
  - actor attribution is correct;
  - no required check exception can auto-merge;
  - disabling feature flag restores manual behavior.

## F2-503 — Normalize structured agent questions

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend + frontend
- **Dependencies:** F2-200, F2-105
- **Code targets:**
  - conversation/task comment schema or structured metadata;
  - attention service;
  - task Activity UI;
  - notifications.
- **Deliverables:**
  - question, context, options, recommendation, impact;
  - open/resolved state;
  - answer resumes task context.
- **Acceptance:**
  - questions appear as attention, not generic failures;
  - resolving a question removes attention;
  - answer and decision are audited.

## F2-504 — Add task transfer service

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/execution
- **Independent review:** required
- **Dependencies:** F2-105, F2-400, F2-304
- **Code targets:**
  - recovery/transfer service;
  - runtime and executor compatibility;
  - task action API;
  - audit and tests.
- **Deliverables:**
  - transfer package;
  - continue-workspace strategy;
  - destination-agent validation;
  - same-task execution lock;
  - activity event.
- **Acceptance:**
  - destination agent receives contract, diff, decisions, failures, and validation evidence;
  - task ID and workspace remain stable;
  - concurrent mutating ownership is prevented.

## F2-505 — Add adaptive recovery and escalation recommendations

- **Size:** M
- **Risk:** high
- **Suggested agent:** backend/recovery
- **Dependencies:** F2-504, F2-500
- **Code targets:**
  - recovery service;
  - attention recommendations;
  - agent capability matching.
- **Deliverables:**
  - continue same agent;
  - transfer to specialist;
  - request human decision;
  - split task recommendation;
  - accept exception/cancel options.
- **Acceptance:**
  - recommendations include reasons and do not execute risky transfer automatically by default;
  - exhaustion produces one clear attention item.

### M5 exit gate

- Reviewer and planner agents are conditional rather than mandatory.
- Risk policy is explainable and audited.
- Trusted auto-merge is safely feature-flagged.
- Tasks transfer without losing context.

---

# M6 — Agent and runtime simplification

## F2-600 — Redesign Agents page as a roster

- **Size:** L
- **Risk:** medium
- **Suggested agent:** frontend
- **Independent UI review:** recommended
- **Dependencies:** F2-301, F2-202
- **Code targets:**
  - Agents page/components;
  - agent summary API additions if needed.
- **Deliverables:**
  - availability, current assignment, capabilities, model, concurrency, project access;
  - basic and advanced configuration separation;
  - recent execution health summary.
- **Acceptance:**
  - ordinary user can create/configure an agent without selecting a daemon;
  - advanced executor and runtime settings remain accessible.

## F2-601 — Move runtime management under Settings → Compute

- **Size:** M
- **Risk:** medium
- **Suggested agent:** frontend
- **Dependencies:** F2-205, F2-003
- **Code targets:**
  - navigation/router;
  - Daemons page and detail copy;
  - settings page.
- **Deliverables:**
  - runtime terminology;
  - system IDs secondary;
  - old daemon URLs preserved;
  - connection and capacity UI retained.
- **Acceptance:**
  - runtime page is absent from primary nav;
  - deep links continue working;
  - docs distinguish UI term from backend/API term.

## F2-602 — Add automatic runtime resolution summary

- **Size:** M
- **Risk:** high
- **Suggested agent:** backend/routing
- **Dependencies:** F2-600
- **Code targets:**
  - daemon transport/router;
  - agent service;
  - task launch action;
  - summary DTOs.
- **Deliverables:**
  - candidate evaluation;
  - automatic choice when unambiguous;
  - user-facing explanation when unavailable;
  - preferred runtime override.
- **Acceptance:**
  - normal task launch requires no runtime input;
  - failure identifies affected agent/task and available remedies;
  - security/data-locality constraints are respected.

## F2-603 — Add agent recommendation for task start

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend/routing
- **Dependencies:** F2-202, F2-600
- **Code targets:**
  - agent capacity/capability service;
  - task create/start API;
  - frontend selector.
- **Deliverables:**
  - recommendation based on project access, capabilities, availability, and executor compatibility;
  - manual override;
  - no opaque automatic reassignment after work starts.
- **Acceptance:**
  - recommendation includes reason;
  - unavailable or unauthorized agents are excluded;
  - user can choose Automatic in task creation.

## F2-604 — Update runtime and agent error presentation

- **Size:** S
- **Risk:** low
- **Suggested agent:** frontend/UX
- **Dependencies:** F2-602
- **Code targets:**
  - error mapping;
  - attention cards;
  - task detail and Home.
- **Deliverables:**
  - user-centered messages;
  - technical details expandable;
  - remediation actions.
- **Acceptance:**
  - no primary error consists only of daemon ID, heartbeat count, or transport code;
  - operator diagnostics retain exact data.

### M6 exit gate

- Agents feel like task assignees.
- Runtime routing is automatic in the normal path.
- Infrastructure remains inspectable under advanced settings.

---

# M7 — Migration and release

## F2-700 — Build project migration preview service

- **Size:** L
- **Risk:** high
- **Suggested agent:** backend/migration
- **Independent review:** required
- **Dependencies:** F2-103, F2-001
- **Code targets:**
  - workflow migration service;
  - API routes;
  - fixtures and tests.
- **Deliverables:**
  - state mapping preview;
  - active task and hook compatibility analysis;
  - role-assignment impact;
  - safe/unsafe status and reasons.
- **Acceptance:**
  - no project is mutated during preview;
  - custom hooks referencing removed states are detected;
  - preview works against legacy fixture.

## F2-701 — Implement atomic project migration to autonomous_v1

- **Size:** L
- **Risk:** critical
- **Suggested agent:** senior backend/migration
- **Independent review:** mandatory
- **Dependencies:** F2-700
- **Code targets:**
  - migration service;
  - database transaction;
  - audit log;
  - integration tests.
- **Deliverables:**
  - atomic workflow and task-state mapping;
  - migration report;
  - rollback on any failure;
  - force path requiring explicit reason and authorization.
- **Acceptance:**
  - task history, executions, workspaces, comments, reviews, and transitions remain accessible;
  - active unsafe tasks block default migration;
  - fixture comparison proves no orphaned records.

## F2-702 — Add migration UI

- **Size:** M
- **Risk:** medium
- **Suggested agent:** frontend
- **Dependencies:** F2-700, F2-701
- **Code targets:**
  - Project Settings → Workflow;
  - preview and report components;
  - E2E tests.
- **Deliverables:**
  - explain benefits and behavior changes;
  - show each state mapping and blocker;
  - recommend waiting for active tasks where appropriate.
- **Acceptance:**
  - user understands whether migration changes active work;
  - force option is advanced and guarded;
  - completed migration updates board immediately.

## F2-703 — Reposition README and getting-started documentation

- **Size:** M
- **Risk:** low
- **Suggested agent:** product/documentation
- **Dependencies:** M2 and M3 complete
- **Code targets:**
  - `README.md`;
  - `docs/getting-started.md`;
  - architecture/concepts docs;
  - screenshots and demo.
- **Deliverables:**
  - headline around reliable task delegation and verified delivery;
  - first task walkthrough using autonomous workflow;
  - advanced strict/custom workflow documentation;
  - runtime terminology clarification.
- **Acceptance:**
  - user can understand product without learning daemon/workflow internals;
  - screenshots match current UI;
  - old docs do not incorrectly describe default planner/coder/reviewer behavior.

## F2-704 — Add local product metrics and UX instrumentation

- **Size:** M
- **Risk:** medium
- **Suggested agent:** backend/analytics
- **Dependencies:** M3, M4
- **Code targets:**
  - local analytics/service;
  - settings/analytics page;
  - privacy documentation.
- **Deliverables:**
  - one-agent completion;
  - interventions;
  - validation retries;
  - transfers;
  - review and merge outcomes;
  - activation funnel.
- **Acceptance:**
  - metrics work without external telemetry;
  - no prompt or source content is exported;
  - user can inspect and clear local metrics.

## F2-705 — Make autonomous_v1 the new-project default

- **Size:** S
- **Risk:** high
- **Suggested agent:** release/platform
- **Dependencies:** all M1–M6 exit gates, F2-703, F2-704
- **Deliverables:**
  - server default switch behind release flag;
  - release notes;
  - rollback configuration;
  - smoke tests on clean installation.
- **Acceptance:**
  - new project receives autonomous_v1 and Standard policy;
  - existing project remains unchanged;
  - disabling flag restores prior default;
  - install/demo path succeeds.

## F2-706 — Deprecation audit for default-role assumptions

- **Size:** M
- **Risk:** medium
- **Suggested agent:** maintenance/test
- **Dependencies:** F2-705
- **Code targets:**
  - prompts;
  - default role constants;
  - UI copy;
  - docs;
  - tests.
- **Deliverables:**
  - remove assumptions that every project has planner/coder/reviewer;
  - retain compatibility helpers for strict workflow;
  - document deprecation timeline for obsolete UI paths.
- **Acceptance:**
  - autonomous project works with worker-only role set;
  - strict project still works;
  - grep/audit finds no accidental mandatory planner assignment.

### M7 exit gate

- New installations default to autonomous_v1 safely.
- Legacy projects are unchanged unless migrated.
- Documentation, screenshots, metrics, rollback, and migration tooling are complete.

---

## 3. Parallel execution opportunities

After M1 API contracts stabilize, the following can run concurrently:

- Home API and Home frontend after response shape is agreed;
- Work API and Work frontend after fixture/mock contract is agreed;
- task detail IA frontend shell while delivery backend is implemented;
- agent roster frontend and runtime-routing backend;
- documentation updates near the end of M3 using feature-flagged screenshots.

Do not parallelize changes that edit the same core transition/workflow files unless they are split into explicit stacked branches.

---

## 4. Recommended first execution batch

Start with exactly these tasks:

1. **F2-001** — compatibility fixtures.
2. **F2-100** — canonical phase.
3. **F2-102** — autonomous workflow preset.
4. **F2-101** — task response/query phase support after F2-100.
5. **F2-104** — task intent façade after F2-102.
6. **F2-002** — typed actor in parallel only with careful ownership because it touches transition internals.

The first product demonstration should occur before task contracts or risk automation:

```text
Create autonomous project
→ create title-only task
→ assign one worker
→ worker completes end to end
→ Forge runs hard checks
→ human reviews
→ merge
```

This validates the strategic direction with the least irreversible schema work.

---

## 5. Milestone review questions

At the end of each milestone, reviewers should answer:

1. Did this reduce concepts or add another layer users must understand?
2. Does the new path preserve external verification and auditability?
3. Can existing strict/custom projects still operate?
4. Is human attention requested only for an actual decision or exception?
5. Can a failed run recover without resetting the durable task?
6. Does the UI explain work using tasks, agents, reviews, and evidence rather than internal process vocabulary?
7. Are policy decisions explainable and testable?
8. Is the next milestone still necessary, or did improved agent behavior eliminate part of the planned orchestration?

