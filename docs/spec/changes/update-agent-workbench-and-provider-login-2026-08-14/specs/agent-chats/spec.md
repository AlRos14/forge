## MODIFIED Requirements

### Requirement: Chat Switcher and Global Launcher

The React application SHALL represent agent-chat scope in the application shell rather than with a second chat-local switcher. It SHALL render the account-level `Main Chat` entry immediately below the Project switcher and immediately above the `Project` section label. It SHALL render the selected Project's `Agent Workspace` within that Project section and SHALL NOT render a global/Project chat roster inside either chat screen. The persistent global launcher SHALL open the same canonical Main Agent timeline as `/chat` and SHALL NOT create separate modal-only history.

#### Scenario: Main Chat has account-level placement

- **WHEN** a user opens the application with any Project selected
- **THEN** the sidebar shows `Main Chat` directly after the Project switcher and before the `Project` section label
- **AND** `Main Chat` is not duplicated in the `Workspace` section

#### Scenario: Project chat is a Project workbench entry

- **WHEN** a user views a selected Project's navigation
- **THEN** the Project section shows one `Agent Workspace` entry for that Project's canonical Project Agent chat
- **AND** changing the selected Project changes the Project Agent scope without changing the account-level Main Chat identity

#### Scenario: Chat pages do not duplicate scope navigation

- **WHEN** a user opens the Main Chat or a Project Agent Workspace
- **THEN** the page does not render an additional list or switcher containing global and Project chats
- **AND** the application shell remains the canonical scope navigator

#### Scenario: Global launcher reuses the canonical timeline

- **WHEN** the user opens the persistent global launcher while viewing any Project surface
- **THEN** it opens the same account-scoped Main Agent timeline used by `/chat`
- **AND** no Project-private context is attached unless the user explicitly supplies an authorized typed Project reference

#### Scenario: Compact navigation preserves hierarchy and access

- **WHEN** the viewport uses the compact navigation drawer
- **THEN** `Main Chat` retains the same relative order before the Project section
- **AND** Main Chat, Agent Workspace, and launcher controls are keyboard reachable, visibly focused, and correctly named for assistive technology
