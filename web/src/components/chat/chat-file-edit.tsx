import { FileCode } from '@phosphor-icons/react'

import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import type { ChatFileEditEntry } from '@/components/chat/types'
import { cn } from '@/lib/cn'

type ChatFileEditProps = {
  entry: ChatFileEditEntry
}

function capitalize(value: string): string {
  return `${value.slice(0, 1).toUpperCase()}${value.slice(1)}`
}

function getDiffStats(entry: ChatFileEditEntry): { additions: number; deletions: number } {
  if (entry.additions !== undefined || entry.deletions !== undefined) {
    return {
      additions: entry.additions ?? 0,
      deletions: entry.deletions ?? 0,
    }
  }

  if (!entry.diff) {
    return { additions: 0, deletions: 0 }
  }

  return entry.diff.split('\n').reduce(
    (stats, line) => {
      if (line.startsWith('+') && !line.startsWith('+++')) {
        stats.additions += 1
      }

      if (line.startsWith('-') && !line.startsWith('---')) {
        stats.deletions += 1
      }

      return stats
    },
    { additions: 0, deletions: 0 },
  )
}

function getDiffLineClassName(line: string): string {
  if (line.startsWith('@@')) {
    return 'text-cyan-600 dark:text-cyan-400'
  }

  if (line.startsWith('+')) {
    return 'text-green-600 dark:text-green-400'
  }

  if (line.startsWith('-')) {
    return 'text-destructive'
  }

  return 'text-muted-foreground'
}

export function ChatFileEdit({ entry }: ChatFileEditProps) {
  const action = capitalize(entry.action)
  const target = entry.action === 'rename' && entry.newPath ? `Rename → ${entry.newPath}` : entry.path
  const stats = getDiffStats(entry)

  return (
    <ChatEntryContainer
      variant="tool"
      status={entry.status}
      icon={<FileCode className="h-4 w-4 text-muted-foreground" aria-hidden="true" />}
      header={
        <div className="flex min-w-0 items-center gap-2">
          {entry.action === 'rename' && entry.newPath ? (
            <span className="min-w-0 truncate font-semibold">{target}</span>
          ) : (
            <>
              <span className="shrink-0 font-semibold">{action}</span>
              <span className="min-w-0 truncate font-mono text-xs font-normal text-muted-foreground">
                {entry.path}
              </span>
            </>
          )}
          <span className="shrink-0 font-mono text-xs text-green-600 dark:text-green-400">+{stats.additions}</span>
          <span className="shrink-0 font-mono text-xs text-destructive">-{stats.deletions}</span>
        </div>
      }
      defaultCollapsed={true}
    >
      {entry.diff ? (
        <pre className="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed">
          {entry.diff.split('\n').map((line, index) => (
            <span
              key={index}
              className={cn('block whitespace-pre-wrap break-words', getDiffLineClassName(line))}
            >
              {line || ' '}
            </span>
          ))}
        </pre>
      ) : entry.before !== undefined || entry.after !== undefined ? (
        <p className="text-sm text-muted-foreground">File content changed (no diff available)</p>
      ) : (
        <pre className="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed text-muted-foreground">
          {JSON.stringify(entry.payload, null, 2)}
        </pre>
      )}
    </ChatEntryContainer>
  )
}
