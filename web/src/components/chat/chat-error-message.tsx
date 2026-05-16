import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import type { ChatErrorEntry } from '@/components/chat/types'

type ChatErrorMessageProps = {
  entry: ChatErrorEntry
}

export function ChatErrorMessage({ entry }: ChatErrorMessageProps) {
  return (
    <ChatEntryContainer variant="error" header={entry.title} defaultCollapsed={false}>
      {entry.message ? (
        <p className="mb-2 whitespace-pre-wrap text-sm text-red-700 dark:text-red-300">
          {entry.message}
        </p>
      ) : null}
      <pre className="max-h-96 overflow-auto rounded-md bg-background p-3 font-mono text-xs text-foreground">
        {JSON.stringify(entry.payload, null, 2)}
      </pre>
    </ChatEntryContainer>
  )
}
