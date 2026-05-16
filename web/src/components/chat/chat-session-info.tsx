import { Info } from '@phosphor-icons/react'

import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import type { ChatSessionInfoEntry } from '@/components/chat/types'

type ChatSessionInfoProps = {
  entry: ChatSessionInfoEntry
}

function toContainerStatus(status: ChatSessionInfoEntry['status']) {
  return status === 'pending' || status === 'success' || status === 'failed' ? status : undefined
}

function formatRelativeTime(value?: string) {
  if (!value) {
    return 'unknown start'
  }

  const timestamp = new Date(value).getTime()

  if (Number.isNaN(timestamp)) {
    return 'unknown start'
  }

  const seconds = Math.round((timestamp - Date.now()) / 1000)
  const absoluteSeconds = Math.abs(seconds)
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ['year', 60 * 60 * 24 * 365],
    ['month', 60 * 60 * 24 * 30],
    ['week', 60 * 60 * 24 * 7],
    ['day', 60 * 60 * 24],
    ['hour', 60 * 60],
    ['minute', 60],
    ['second', 1],
  ]
  const [unit, unitSeconds] = units.find(([, unitSeconds]) => absoluteSeconds >= unitSeconds) ?? ['second', 1]

  return new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' }).format(Math.round(seconds / unitSeconds), unit)
}

export function ChatSessionInfo({ entry }: ChatSessionInfoProps) {
  const sessionId = entry.sessionId ? entry.sessionId.slice(0, 8) : 'unknown'
  const agent = entry.agent ?? 'Agent'
  const startedAt = formatRelativeTime(entry.startedAt)

  return (
    <ChatEntryContainer
      variant="session"
      status={toContainerStatus(entry.status)}
      icon={<Info weight="duotone" />}
      header={
        <div className="flex min-w-0 items-center gap-2 text-sm">
          <span className="truncate font-mono text-xs text-muted-foreground">{sessionId}</span>
          <span className="truncate font-medium">{agent}</span>
          <span className="shrink-0 text-muted-foreground">{startedAt}</span>
        </div>
      }
      defaultCollapsed
      children={null}
    />
  )
}
