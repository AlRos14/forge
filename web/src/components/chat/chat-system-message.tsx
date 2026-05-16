import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import type { ChatSystemEntry } from '@/components/chat/types'

type ChatSystemMessageProps = {
  entry: ChatSystemEntry
}

export function ChatSystemMessage({ entry }: ChatSystemMessageProps) {
  return (
    <ChatEntryContainer variant="system" header={entry.title} defaultCollapsed={true}>
      <pre className="max-h-96 overflow-auto rounded-md bg-muted p-3 font-mono text-xs text-foreground">
        {JSON.stringify(entry.payload, null, 2)}
      </pre>
    </ChatEntryContainer>
  )
}
