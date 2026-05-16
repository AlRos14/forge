import { TerminalWindow } from '@phosphor-icons/react'
import { FancyAnsi } from 'fancy-ansi'

import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import type { ChatShellOutputEntry } from '@/components/chat/types'
import { cn } from '@/lib/cn'

type ChatShellOutputProps = {
  entry: ChatShellOutputEntry
}

const ansi = new FancyAnsi()

function convertAnsiToHtml(value: string): string {
  return ansi.toHtml(value)
}

export function ChatShellOutput({ entry }: ChatShellOutputProps) {
  return (
    <ChatEntryContainer
      variant="tool"
      status={entry.status}
      icon={<TerminalWindow className="h-4 w-4 text-muted-foreground" aria-hidden="true" />}
      header={
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate font-semibold">{entry.command ?? 'Shell output'}</span>
          {entry.command && entry.cwd ? (
            <span className="min-w-0 truncate text-xs font-normal text-muted-foreground">{entry.cwd}</span>
          ) : null}
        </div>
      }
      defaultCollapsed={true}
    >
      {entry.lines.length > 0 ? (
        <pre className="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed text-muted-foreground">
          {entry.lines.map((line, index) => (
            <span
              key={`${line.stream}-${index}`}
              className={cn(
                'block whitespace-pre-wrap break-words',
                line.stream === 'stderr' && 'text-destructive',
              )}
              dangerouslySetInnerHTML={{ __html: convertAnsiToHtml(line.text) }}
            />
          ))}
        </pre>
      ) : (
        <p className="text-xs text-muted-foreground">Command produced no output.</p>
      )}
    </ChatEntryContainer>
  )
}
