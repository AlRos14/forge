# Documentation audit — post-V2 hardening

Date: 2026-10-28. Base: `origin/main` at
`7fb78d9e845869122e61cce5cb510a3b0853afc9` (PR #20).

Classification: **A** current and correct; **B** historical and clearly marked;
**C** obsolete but presented as operational; **D** contradictory with current
code; **E** broken link/reference; **F** redundant artifact.

| Path / group | Class before | Decision | Change |
|---|---|---|---|
| `README.md` | A | Keep short, update current facts | Added V135, container/security/deployment links. |
| `CLAUDE.md` / `AGENTS.md` | A | Update agent guidance | INV-040, current auth/owner rules, V135, deployment link. |
| `docs/architecture.md` | A | Update current runtime description | Non-root container, current exposure, V135, no retired operational surfaces. |
| `docs/api.md` | A | Update public contract | Query-token rejection, SSE ticket, current JWT/OAuth authority, INV-040 and V135. |
| `docs/cli.md` | A | Update `forge-ctl` contract | MCP install writes Bearer header; tokenized URLs must be reinstalled. |
| `docs/getting-started.md` | A | Verify | No query credentials, retired commands, or distribution claims found. |
| `docs/deployment.md` | — | Add | Container identity, root-owned volume migration, loopback default, public-exposure checklist, failure behavior. |
| `docs/security-audit.md` | — | Add | Cargo/pnpm audit results, CodeQL/Scorecard triage, justified non-vulnerability exceptions, private-intake pending action. |
| `docs/release_checklist.md` | A | Update | Non-root/loopback checks, reproducible local release-candidate commands. |
| `docs/execution-logs.md` | A | Keep | Execution/log contract remains current; title's “Chat History” is historical naming only. |
| `docs/upstream.md` | A | Keep | Attribution and independent-maintenance contract remain correct. |
| `docs/concepts/*` | A | Keep | Actor, role, Execution, WorkUnit, Gate, review, validation, collaboration, and session contracts match runtime. Historical Agent Chat storage is labeled. |
| `docs/adr/*` | A | Keep | Accepted architecture decisions remain current evidence. |
| `docs/architecture-review-task-actor.md` | B | Keep | Already has a historical banner; describes retired WorkflowEngine. |
| `docs/glue-code-audit-2026-07-01.md` | C | Mark historical | Added banner; findings are baseline evidence, not current guidance. |
| `docs/prompt-design-report.md` | C | Mark historical | Added banner; prompt builders/workflow dispatch are retired. |
| `docs/synapsis-memory-report.md` | C | Mark historical | Added banner; scoped memory/Agent Chat are retired. |
| `docs/migration/architecture-v2.md` | A/B | Keep as current invariant contract plus historical ledger | Updated status, added INV-040 and V135; historical sections remain labeled. |
| `docs/migration/plan-pr*.md`, `pr-*.md` | B/C | Mark historical | Added a uniform migration-ledger banner; statuses like “not merged” are authoring-time text. |
| `docs/migration/README.md` | — | Add | Index and classification for migration documents. |
| `docs/migration/post-v2-hardening-acceptance.md` | — | Add | Current hardening ledger, defect matrix, and A/B/C evidence boundaries. |
| `docs/migration/history-v071-v076-project-os.md` | B | Keep | Clearly historical Project OS provenance. |
| `docs/migration/plan-pr15-*.md` | B | Keep with current note | Added post-merge status note; pinned evidence remains historical. |
| `docs/spec/README.md` | C/E | Rewrite | Existing `project.md` and `specs/` links did not exist. New index identifies current contracts and historical packages. |
| `docs/spec/forge-v2-autonomous-work-management/*` | C/D | Mark historical | Added banner. Its retained workflow engine, single-owner-agent, Attention, and Genesis proposals are not current. |
| `docs/spec/changes/add-project-agent-federation-*` | C | Mark historical | Added retired-proposal banner. |
| `docs/spec/changes/add-project-charter-milestones-*` | C | Mark historical | Added banner; Charter/Genesis/milestone operations retired, ProjectRelease read-only. |
| `docs/spec/changes/update-agent-workbench-and-provider-login-*` | C | Mark historical | Added partially-retired banner; current Agent/provider docs pointed to. |
| `.codex/skills/forge-main-agent/` | C | Remove from discovery | Deleted. Git history preserves the retired skill. |
| `.codex/skills/forge-project-agent/` | C | Remove from discovery | Deleted. Git history preserves the retired skill. |
| `.codex/skills/forge-orchestrate-projects/` | C | Remove from discovery | Deleted. Git history preserves the retired skill and templates. |
| `DESIGN.md` | A | Keep | Current web design system, not a legacy runtime document. |
| `CONTRIBUTING.md`, `web/README.md`, `web/e2e/README.md` | A | Keep | Build/e2e instructions match the current source layout. |
| `.github/PULL_REQUEST_TEMPLATE.md`, issue templates, `SECURITY.md` | A | Update `SECURITY.md` only | Documented the exact pending private-reporting setting; no invented contact. |
| `npx-cli/README.md` | A/B | Keep | Clearly marked upstream-owned; not a fork distribution channel. |
| `CHANGELOG.md` | A | Update | Added security, fixed, documentation, and breaking entries under Unreleased. |

## Legacy reference decisions

- `coder → implementer` is retained only as a documented migration alias in
  [concepts/roles.md](concepts/roles.md); it is not a current role vocabulary.
- PR2 session fallback and `workflows_dir` appear only in historical migration
  ledgers or as removed compatibility evidence. They are not active
  instructions.
- Retired `WorkflowEngine`, Agent Chat, Attention, Genesis, Project OS, and
  Main/Project Agent references remain only in marked history or explicit
  “retired” statements in current concepts.
- ForgeAILab npm/Homebrew/install artifacts remain attributed upstream material,
  never this fork's distribution.

## Verification

Relative Markdown links under `README.md`, `CLAUDE.md`, `CONTRIBUTING.md`,
`docs/`, and `.github/` were checked with a local relative-link scan; no broken
targets remained after the index rewrite. This scan does not validate external
URLs or generated web routes.
