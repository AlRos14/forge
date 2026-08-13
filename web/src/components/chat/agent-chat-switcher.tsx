import { ChatCircleDots, Robot } from '@phosphor-icons/react'
import { Avatar } from '@/components/ui/avatar'
import { cn } from '@/lib/cn'
import { SectionKicker, StatusDot } from '@/features/federation/components'

export type ChatSwitcherEntry = {
  id: string
  label: string
  description: string
  agentName?: string | null
  agentStatus?: string | null
  setupRequired?: boolean
  pendingTurnCount?: number
  active?: boolean
}

function ChatEntry({ entry, onSelect }: { entry: ChatSwitcherEntry; onSelect: () => void }) {
  const identityLabel = entry.setupRequired
    ? 'Setup required'
    : (entry.agentName ?? 'Agent binding configured')
  const pendingLabel =
    entry.pendingTurnCount && entry.pendingTurnCount > 0
      ? `${entry.pendingTurnCount} turn${entry.pendingTurnCount === 1 ? '' : 's'} pending`
      : null

  return (
    <li>
      <button
        type="button"
        className={cn(
          'flex min-h-14 w-full min-w-0 items-center gap-2 rounded-lg border px-2.5 py-2 text-left transition-colors',
          'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
          entry.active
            ? 'border-ember-border bg-ember-surface text-foreground'
            : 'border-transparent text-muted-foreground hover:border-border-subtle hover:bg-muted/40 hover:text-foreground',
        )}
        aria-current={entry.active ? 'page' : undefined}
        aria-label={`${entry.label}: ${entry.description}`}
        onClick={onSelect}
      >
        <Avatar
          name={entry.setupRequired ? 'Setup' : (entry.agentName ?? entry.label)}
          seed={entry.id}
          size="sm"
          className="shrink-0"
        />
        <span className="min-w-0 flex-1">
          <span className="flex min-w-0 items-center gap-1.5">
            <span className="truncate text-xs font-semibold">{entry.label}</span>
            {!entry.setupRequired && entry.agentStatus ? (
              <StatusDot status={entry.agentStatus} />
            ) : null}
          </span>
          <span className="mt-0.5 block truncate text-micro text-muted-foreground">
            {entry.setupRequired ? identityLabel : (pendingLabel ?? entry.description)}
          </span>
        </span>
      </button>
    </li>
  )
}

export function AgentChatSwitcher({
  globalEntry,
  projectEntries,
  onSelectGlobal,
  onSelectProject,
}: {
  globalEntry: ChatSwitcherEntry
  projectEntries: ChatSwitcherEntry[]
  onSelectGlobal: () => void
  onSelectProject: (entry: ChatSwitcherEntry) => void
}) {
  return (
    <aside
      aria-label="Agent chat switcher"
      className="flex min-h-0 w-full shrink-0 flex-col border-b border-border-subtle bg-background lg:w-64 lg:border-b-0 lg:border-r"
    >
      <header className="border-b border-border-subtle px-4 py-4">
        <div className="flex items-center gap-2">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-ember-surface text-primary">
            <Robot size={16} aria-hidden />
          </div>
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold text-foreground">Agent chat</h2>
            <p className="mt-0.5 text-xs text-muted-foreground">One timeline per owning scope</p>
          </div>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-4 lg:space-y-5">
        <section aria-labelledby="global-chat-heading">
          <div className="px-1 pb-2">
            <SectionKicker>
              <span id="global-chat-heading">Global</span>
            </SectionKicker>
            <p className="mt-1 px-1 text-xs leading-4 text-muted-foreground">
              Account-owned Main Agent chat
            </p>
          </div>
          <ul className="space-y-1">
            <ChatEntry entry={globalEntry} onSelect={onSelectGlobal} />
          </ul>
        </section>

        <section aria-labelledby="project-chats-heading" className="mt-4 lg:mt-0">
          <div className="flex items-start justify-between gap-2 px-1 pb-2">
            <div className="min-w-0">
              <SectionKicker>
                <span id="project-chats-heading">Projects</span>
              </SectionKicker>
              <p className="mt-1 px-1 text-xs leading-4 text-muted-foreground">
                One Project Agent chat per Project
              </p>
            </div>
            <ChatCircleDots
              size={15}
              className="mt-0.5 shrink-0 text-muted-foreground"
              aria-hidden
            />
          </div>
          {projectEntries.length > 0 ? (
            <ul className="space-y-1">
              {projectEntries.map((entry) => (
                <ChatEntry key={entry.id} entry={entry} onSelect={() => onSelectProject(entry)} />
              ))}
            </ul>
          ) : (
            <p className="rounded-lg border border-dashed border-border-subtle bg-muted/20 px-3 py-3 text-xs text-muted-foreground">
              No Project Agent chats are available yet.
            </p>
          )}
        </section>
      </div>
    </aside>
  )
}
