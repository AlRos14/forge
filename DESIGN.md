# Forge Design System

## 1. Atmosphere & Identity

Forge is a focused foundry: calm, compact operational surfaces in warm stone and charcoal, with an ember-orange accent that signals action and live work. Its signature is the ember edge—a restrained orange rail, glow, or focus treatment that makes active work legible without turning the board into a decorative dashboard.

## 2. Color

Colors are implemented as HSL component custom properties in `web/src/index.css` and consumed through semantic Tailwind names. Alpha variants of these tokens are allowed; new raw colors are not.

### Palette

| Role           | Token                                         |                    Light |                      Dark | Usage                                                         |
| -------------- | --------------------------------------------- | -----------------------: | ------------------------: | ------------------------------------------------------------- |
| Canvas         | `background` / `--background`                 |              `60 9% 98%` |               `20 13% 5%` | Application and board canvas                                  |
| Text           | `foreground` / `--foreground`                 |             `24 10% 10%` |               `60 9% 98%` | Primary text and icons                                        |
| Card           | `card` / `--card`                             |              `0 0% 100%` |              `24 10% 10%` | Cards and main content                                        |
| Card text      | `card-foreground` / `--card-foreground`       |             `24 10% 15%` |               `60 9% 98%` | Text on cards                                                 |
| Muted surface  | `muted` / `--muted`                           |             `30 12% 95%` |               `20 9% 15%` | Subdued controls and metadata                                 |
| Muted text     | `muted-foreground` / `--muted-foreground`     |              `25 6% 40%` |               `24 5% 45%` | Secondary labels and disabled text                            |
| Border         | `border` / `--border`                         |             `30 10% 88%` |               `18 6% 21%` | Inputs and strong separators                                  |
| Subtle border  | `border-subtle` / `--border-subtle`           |             `30 10% 92%` |               `10 7% 15%` | Cards and low-contrast panels                                 |
| Input          | `input` / `--input`                           |             `30 10% 88%` |               `18 6% 21%` | Input outlines                                                |
| Focus          | `ring` / `--ring`                             |             `25 95% 53%` |              `25 95% 53%` | Keyboard focus indication                                     |
| Primary ember  | `primary` / `--primary`                       |             `21 90% 40%` |              `25 95% 53%` | Primary actions and active work; calibrated for WCAG contrast |
| Primary text   | `primary-foreground` / `--primary-foreground` |              `0 0% 100%` |              `24 10% 10%` | Text on primary controls                                      |
| Secondary      | `secondary` / `--secondary`                   |             `30 12% 95%` |               `20 9% 15%` | Secondary controls                                            |
| Accent         | `accent` / `--accent`                         |             `30 12% 93%` |               `20 9% 15%` | Hover and selected surfaces                                   |
| Destructive    | `destructive` / `--destructive`               |              `0 84% 60%` |               `0 72% 51%` | Errors and destructive actions                                |
| Success        | `success` / `--success`                       |            `160 84% 39%` |             `160 84% 39%` | Completed and healthy states                                  |
| Warning        | `warning` / `--warning`                       |             `38 92% 50%` |              `38 92% 50%` | Caution and recoverable conflict                              |
| Popover        | `popover` / `--popover`                       |              `0 0% 100%` |              `24 10% 10%` | Menus, tooltips, dialogs                                      |
| Sidebar        | `sidebar` / `--sidebar`                       |              `0 0% 100%` |               `20 13% 4%` | Navigation shell                                              |
| Sidebar hover  | `sidebar-hover` / `--sidebar-hover`           |             `30 12% 95%` |               `20 9% 15%` | Navigation hover state                                        |
| Sidebar active | `sidebar-active` / `--sidebar-active`         |             `25 95% 53%` |              `25 95% 53%` | Active navigation marker                                      |
| Ember surface  | `ember-surface` / `--ember-surface`           | `rgba(234, 88, 12, .08)` | `rgba(249, 115, 22, .08)` | Quiet active backgrounds                                      |
| Ember border   | `ember-border` / `--ember-border`             | `rgba(234, 88, 12, .22)` | `rgba(249, 115, 22, .22)` | Active borders                                                |

### Rules

- Use semantic Tailwind tokens rather than literal colors in components.
- Ember is reserved for primary action, focus, active navigation, current work, and drag/drop intent.
- Destructive red communicates failure; warning communicates a stale-board conflict that can be reconciled.
- Light and dark modes must expose the same semantic hierarchy and interaction states.

## 3. Typography

### Font stacks

- Primary: `Inter, system-ui, -apple-system, sans-serif`.
- Mono: `JetBrains Mono, Fira Code, ui-monospace, monospace`.
- No serif family is used.

### Scale

| Token / utility | Size | Line height | Typical use                                          |
| --------------- | ---: | ----------: | ---------------------------------------------------- |
| `text-micro`    | 10px |         1.2 | Uppercase column labels, counters, compact metadata  |
| `text-xs`       | 12px |        1rem | Secondary labels and dense controls                  |
| `text-ui`       | 13px |        1.45 | Default board cards, menus, and operational controls |
| `text-sm`       | 14px |     1.25rem | Body copy and standard controls                      |
| `text-base`     | 16px |      1.5rem | Dialog titles and emphasized body copy               |
| `text-lg`       | 18px |     1.75rem | Section headings                                     |
| `text-page`     | 22px |         1.3 | Page title                                           |
| `text-2xl`      | 24px |        2rem | Large card/page headings where already established   |

Weights are regular 400, medium 500, semibold 600, and bold 700. Operational overlines use the mono family, semibold weight, uppercase text, and `0.8px` to `1.2px` tracking.

## 4. Spacing & Layout

### Base unit

All new spacing is based on 4px. Existing 2px and 6px compact gaps are accepted legacy half-step values and must not spread into new layout primitives.

| Token       | Value | Usage                                        |
| ----------- | ----: | -------------------------------------------- |
| `space-0.5` |   2px | Existing dense list separation only          |
| `space-1`   |   4px | Icon insets and tight groups                 |
| `space-1.5` |   6px | Existing card metadata gaps                  |
| `space-2`   |   8px | Compact control and card spacing             |
| `space-2.5` |  10px | Existing dense card padding                  |
| `space-3`   |  12px | Inputs and toolbar groups                    |
| `space-4`   |  16px | Mobile page/board padding and dialog spacing |
| `space-5`   |  20px | Desktop content padding                      |
| `space-6`   |  24px | Comfortable panel padding                    |
| `space-8`   |  32px | Major component separation                   |

### Shell and board geometry

- Viewport shell: `min-height: 100vh` fallback plus `100dvh`; the page itself never owns horizontal overflow.
- Shell modes: full 240px navigation at `>=1440px`; 56px rail from `1024px` through `1439px`; closed overlay drawer below `1024px`.
- A user-expanded rail may temporarily render the full navigation without rewriting the persisted wide-desktop preference.
- Board route main: `min-width: 0`, no scrolling, and no generic page padding. Other routes retain the existing 20px content padding and main scroll.
- Board page: toolbar is fixed above a single `min-height: 0` board viewport. The viewport owns both horizontal and vertical drag scrolling.
- Columns: `min-width: 220px` at 1280px, a comfortable tablet width that allows at least two columns at 768px, and `min-width: 280px` at 375px. Column/task-list children never establish another scroll container.
- Board padding/gaps use 16px at mobile/tablet and 20px at desktop, with 8px to 12px column gaps.
- Card width follows its column and must remain at least 280px on a 375px viewport after board padding.

## 5. Components

### Button and icon button

- **Structure:** semantic `button`, optional Phosphor icon, label or accessible name.
- **Variants:** primary, destructive, outline, secondary, ghost, link; text and icon sizes.
- **States:** default, hover, active press, visible focus ring, disabled opacity/cursor, pending/busy.
- **Accessibility:** icon-only controls require an accessible name; disabled state uses native `disabled` where possible.
- **Motion:** color/opacity/transform only, using the micro timing token.

### Form controls

- **Structure:** label plus input/select/textarea/checkbox/switch, supporting help and error text.
- **States:** default, hover where relevant, focus ring, disabled, invalid, loading where asynchronous.
- **Accessibility:** programmatic label, described errors, keyboard operation, and contrast in both themes.

### Card and panel

- **Structure:** semantic section/article with optional header, content, metadata, and actions.
- **Variants:** standard card, compact operational card, elevated popover/dialog.
- **States:** default, hover elevation, active/selected ember treatment, disabled/muted, loading skeleton, empty, error.
- **Depth:** subtle border plus tokenized shadow; do not add isolated shadow recipes.

### App shell navigation

- **Structure:** skip link, navigation, header, and main landmark.
- **Variants:** full sidebar, compact rail, overlay drawer.
- **States:** active item, hover, focus, drawer open/closed, persisted desktop collapse preference.
- **Accessibility:** drawer traps/contains focus through existing dialog/sheet behavior, closes on Escape and outside click, and returns focus to its menu trigger.
- **Motion:** drawer/rail transitions use standard timing, transform, and opacity; reduced motion removes non-essential movement.

### Board toolbar

- **Structure:** page identity, search/filter controls, create action, and a polite status/explanation region.
- **States:** default, filtering, ordering-disabled, committing, conflict, loading, and error.
- **Accessibility:** ordering eligibility is visible text and announced; non-ordering controls remain usable during a move.

### Board viewport and column

- **Structure:** one scroll-owning board viewport containing non-scrolling droppable columns.
- **States:** loading skeleton, empty board/column, error, valid drop target, active drop target, ordering-disabled, and committing.
- **Accessibility:** named regions/columns; board status is announced without stealing focus.
- **Responsive:** follows the column widths and padding in Section 4 without document overflow.

### Kanban task card

- **Structure:** draggable article, detail-navigation body, status/assignment metadata, dedicated drag handle, and overflow menu.
- **States:** default, hover, active press, keyboard focus within, dragging, committing/busy, disabled ordering, blocked/error, terminal-muted.
- **Accessibility:** the card body and drag handle are separate targets; the handle is a visible button-like control with an accessible name and at least a 32px target. Keyboard drag uses the DnD library controls.
- **Motion:** hover/drag uses tokenized shadow plus transform/opacity; active in-progress ember motion respects reduced motion.

### Drag handle

- **Structure:** Phosphor grip icon in a dedicated 32px control; only this control receives `dragHandleProps`.
- **States:** subtle default, visible hover, active press, high-contrast focus ring, disabled, and committing/busy.
- **Accessibility:** accessible name includes the task title; native disabled semantics when DnD permits, plus `aria-disabled`/`aria-busy` when committing.

### Conflict notice

- **Structure:** warning-toned status banner with message and optional refresh/retry-safe action.
- **States:** hidden, reconciling, resolved, and persistent error.
- **Copy:** stale moves say “Board changed while you were dragging; refreshed to the latest version.”
- **Accessibility:** `role="status"` for reconciliation and `role="alert"` only when user action is required.

### Agent chat switcher

- **Structure:** one `Global · Main` entry followed by one entry per Project. Each Project entry opens that Project's single Agent Chat; unbound identities never appear here.
- **States:** active, ready, setup required, loading, unavailable, and empty Project list. Setup status stays visible in the entry and in the chat surface.
- **Accessibility:** entries are keyboard-operable buttons with `aria-current="page"` for the active scope, visible focus rings, and a named navigation region. There is no arbitrary identity roster or “new chat” action in this switcher.
- **Responsive:** the switcher is a compact horizontal list below the chat header on narrow screens and a bounded left rail on larger screens; it must not create document-wide horizontal overflow.

### Agent chat timeline and composer

- **Structure:** server-authoritative Agent Chat messages, explicit handoffs, and one composer. Do not derive handoffs or target navigation from message text, task IDs, or retired Room/Conversation aliases.
- **Turn states:** finite `sending`, `queued`, `leased`, `running`, `retry_wait`, `succeeded`, `failed`, and `cancelled` states are visible in the timeline and announced with `role="status"` or `role="alert"` as appropriate.
- **Composer behavior:** Enter submits a non-composing message; Shift+Enter inserts a newline; IME composition is never interrupted. The send control is disabled while the current turn is live or the binding is not ready, with truthful copy explaining why.
- **Handoffs:** a handoff's Continue action opens the target Project Agent Chat and never redirects to a board/task view. Context provenance remains inspectable from the explicit manifest identifier.
- **States:** loading, recoverable error with retry, empty timeline, setup required, pending turn, and settled timeline all have explicit copy and keyboard-visible actions.
- **Refresh:** server events may accelerate updates, but mounted timelines poll messages, turns, handoffs, and chat status at a bounded interval so a completed response never depends on an unavailable event channel.

### Global chat launcher

- **Structure:** a bottom-right launcher opens the same account-owned Main Agent timeline as `/chat`; it never creates a second chat or local fork.
- **Accessibility:** the launcher has an accessible name, Escape closes the panel, focus moves into the panel on open, and focus returns to the launcher on close. The panel is responsive to viewport height and keeps the composer reachable above the safe area.

### Binding setup controls

- **Structure:** Agent settings can connect identities; Main settings select exactly one identity/profile through `/account/main-agent`; Project settings select exactly one identity/profile through `/projects/{id}/project-agent`.
- **Truthfulness:** binding controls show server state and expected version, preserve optimistic-concurrency errors, and display the server-enforced permission ceiling as read-only metadata. Role, primary/steward, participant, archive-membership, and arbitrary capability-grant controls are not part of this surface.

### Mission Control and Agent detail

- **Hierarchy:** primary views lead with the singular Main binding, one Project binding per authorized Project, relevant Task Worker/reviewer activity, Attention, and outcomes. Connected profiles without a binding or active Task scope stay in secondary configuration inventory.
- **Scope isolation:** a Project Agent view requests only its Project's handoff metadata; the Main timeline may show explicit handoff receipts but never imports Project-private history or memory.
- **Recovery:** live chat turns expose a server-versioned “Cancel turn” action using an idempotency key; terminal turns expose only a bounded “Retry turn” action that re-admits the same request through normal server policy. Leased, queued, and retry-wait turns remain server-controlled and do not expose an unbounded client retry.
- **Containment:** long message, identifier, and error content wraps inside the timeline; the timeline owns horizontal clipping and never creates page-level overflow.

## 6. Motion & Interaction

| Token     |  Duration | Easing                          | Usage                                        |
| --------- | --------: | ------------------------------- | -------------------------------------------- |
| Micro     | 100–150ms | `ease-out`                      | Hover, active press, focus visibility, menus |
| Standard  |     200ms | `ease-in-out`                   | Sidebar/rail and drawer state changes        |
| Emphasis  | 400–600ms | `cubic-bezier(0.16, 1, 0.3, 1)` | Reserved for meaningful page-level emphasis  |
| Live work |    2200ms | `ease-in-out`                   | Existing in-progress ember pulse             |

- Animate transform, opacity, filter, color, border color, and tokenized shadow only; never animate layout dimensions or positions.
- Every interactive element has hover, active, and focus-visible treatment.
- Drag start freezes an ID-keyed board snapshot. Updates queue until commit/reconciliation, and a second gesture cannot start while a move is committing.
- Conflicts never auto-retry against newer versions; current server truth replaces the frozen snapshot and the result is announced.
- Respect `prefers-reduced-motion`; all core state meaning remains visible without animation.

## 7. Depth & Surface

Forge uses a **mixed border-and-soft-shadow** strategy. Warm tonal shifts define the shell and column hierarchy; subtle borders define card edges; soft shadows communicate card lift and floating surfaces.

| Level        | Token                                | Usage                                |
| ------------ | ------------------------------------ | ------------------------------------ |
| Hairline     | `border-subtle`                      | Columns and quiet cards              |
| Default      | `border`                             | Inputs and emphasized separators     |
| Rest         | `shadow-xs`, `shadow-soft`           | Controls and cards at rest           |
| Hover        | `shadow-card-hover`                  | Interactive card lift                |
| Floating     | `shadow-float`                       | Menus, dialogs, overlay navigation   |
| Active ember | `shadow-ember`, ember surface/border | Current work, focus, and drag intent |

New surfaces must reuse these levels. The current code contains a few legacy literal status colors, arbitrary compact measurements, and generic `shadow-sm`/`shadow-lg` utilities; they are accepted debt outside this change and should be consolidated only in separately approved work.
