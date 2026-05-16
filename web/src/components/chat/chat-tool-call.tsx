import { Wrench } from '@phosphor-icons/react'
import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import { cn } from '@/lib/cn'
import type { ChatToolCallEntry } from '@/components/chat/types'

type ChatToolCallProps = {
  entry: ChatToolCallEntry
}

const statusDotClassNames = {
  success: 'bg-green-500',
  failed: 'bg-destructive',
  denied: 'bg-destructive',
  timed_out: 'bg-destructive',
}

function formatValue(value: unknown): string {
  if (typeof value === 'string') {
    return value
  }

  return JSON.stringify(value, null, 2) ?? String(value)
}

function StatusDot({ status }: { status: ChatToolCallEntry['status'] }) {
  if (!status) {
    return null
  }

  if (status === 'pending' || status === 'pending_approval') {
    return (
      <span className="relative inline-flex h-2.5 w-2.5 shrink-0" aria-label={status}>
        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-muted-foreground/40" />
        <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-muted-foreground/60" />
      </span>
    )
  }

  return <span className={cn('h-2.5 w-2.5 shrink-0 rounded-full', statusDotClassNames[status])} aria-label={status} />
}

export function ChatToolCall({ entry }: ChatToolCallProps) {
  const defaultCollapsed = true

  return (
    <ChatEntryContainer
      variant="tool"
      status={entry.status}
      icon={<Wrench className="h-4 w-4 text-muted-foreground" aria-hidden="true" />}
      header={
        <span className="flex min-w-0 items-center gap-2">
          <span className="shrink-0">{entry.toolName}</span>
          {entry.inputLabel && (
            <span className="truncate font-mono text-xs text-muted-foreground">{entry.inputLabel}</span>
          )}
          {entry.resultLabel && (
            <span className="min-w-0 truncate text-xs font-normal text-muted-foreground">{entry.resultLabel}</span>
          )}
          <StatusDot status={entry.status} />
        </span>
      }
      defaultCollapsed={defaultCollapsed}
    >
      <div className="space-y-3">
        {entry.input !== undefined ? (
          <pre className="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed text-muted-foreground">
            {formatValue(entry.input)}
          </pre>
        ) : null}
        {entry.result !== undefined ? (
          <pre className="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed text-muted-foreground">
            {formatValue(entry.result)}
          </pre>
        ) : null}
      </div>
    </ChatEntryContainer>
  )
}
