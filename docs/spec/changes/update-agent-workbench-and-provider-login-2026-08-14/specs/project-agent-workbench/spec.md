## ADDED Requirements

### Requirement: Project Agent Workspace Editing Surface

Forge SHALL present each Project Agent chat as an `Agent Workspace` that combines the durable Project Agent timeline with authorized typed editing affordances for the bound Project's records, Decisions, artifacts, milestones, and Tasks. The surface SHALL identify the active Project and Project Agent and SHALL expose pending, saved, conflict, and failed outcomes for user and agent mutations.

#### Scenario: User edits Project data beside the conversation

- **WHEN** an authorized user changes a Project record from the Agent Workspace
- **THEN** Forge applies the change through the corresponding typed Project service with optimistic concurrency
- **AND** the UI displays a durable success receipt or an actionable conflict/error without silently overwriting newer data

#### Scenario: Project Agent manages work through typed actions

- **WHEN** the Project Agent proposes or performs an authorized Project or Task mutation
- **THEN** the action is validated against its bound Project scope and executed through the existing service boundary
- **AND** the resulting typed receipt is linked into the Project Agent timeline

#### Scenario: Workbench adapts to compact screens

- **WHEN** the Agent Workspace is rendered at a compact viewport
- **THEN** conversation and editing affordances remain available through a labeled segmented view
- **AND** changing segments preserves draft state, focus context, and unsaved-change warnings

### Requirement: Project Agent Workspace Repository Boundary

The Agent Workspace SHALL NOT grant the Main Agent or Project Agent a repository root, worktree, shell, raw filesystem tool, or Workspace lease. Repository mutation SHALL remain available only to authorized Task Workers/reviewers through the existing Task Workspace and workflow contracts.

#### Scenario: Project work requires repository changes

- **WHEN** a Project Agent determines that code or filesystem changes are required
- **THEN** it creates or manages a traceable Task through the authorized Task service
- **AND** it does not directly read or mutate repository files from the Project Agent run

#### Scenario: Workbench client attempts a repository operation

- **WHEN** a client submits a repository or shell operation through an Agent Workspace action surface
- **THEN** Forge rejects the operation at the server authority boundary
- **AND** no Workspace lease, path, or repository capability is disclosed
