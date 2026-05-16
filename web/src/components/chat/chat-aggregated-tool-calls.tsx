import { Wrench } from '@phosphor-icons/react'
import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import { ChatToolCall } from './chat-tool-call'
import { cn } from '@/lib/cn'
import type { ChatAggregatedToolCallsEntry } from '@/components/chat/types'

type ChatAggregatedToolCallsProps = {
  entry: ChatAggregatedToolCallsEntry
}

type ChatAggregatedToolCallsEntryWithWorstStatus = ChatAggregatedToolCallsEntry & {
  worstStatus?: ChatAggregatedToolCallsEntry['status']
}

const statusDotClassNames = {
  success: 'bg-green-500',
  failed: 'bg-destructive',
  denied: 'bg-destructive',
  timed_out: 'bg-destructive',
}

function StatusDot({ status }: { status: ChatAggregatedToolCallsEntry['status'] }) {
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

export function ChatAggregatedToolCalls({ entry }: ChatAggregatedToolCallsProps) {
  const worstStatus = (entry as ChatAggregatedToolCallsEntryWithWorstStatus).worstStatus ?? entry.status
  const defaultCollapsed = worstStatus === 'success'

  return (
    <ChatEntryContainer
      variant="tool"
      status={worstStatus}
      icon={<Wrench className="h-4 w-4 text-muted-foreground" aria-hidden="true" />}
      header={
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate">
            {entry.calls.length} x {entry.toolName}
          </span>
          <StatusDot status={worstStatus} />
        </span>
      }
      defaultCollapsed={defaultCollapsed}
    >
      <div className="space-y-2">
        {entry.calls.map((call) => (
          <ChatToolCall key={`${call.sequence}-${call.callId ?? call.toolName}`} entry={call} />
        ))}
      </div>
    </ChatEntryContainer>
  )
}
