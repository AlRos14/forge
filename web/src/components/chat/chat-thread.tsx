import { useEffect, useMemo, useRef, useState } from 'react'
import { ArrowLineUp, ArrowsClockwise } from '@phosphor-icons/react'
import { ChatAssistantMessage } from '@/components/chat/chat-assistant-message'
import { ChatSystemMessage } from '@/components/chat/chat-system-message'
import { ChatUserMessage } from '@/components/chat/chat-user-message'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/cn'
import type { ChatAssistantEntry, ChatSystemEntry, ChatUserEntry } from '@/components/chat/types'
import type { ConversationMessage } from '@/types/generated'

type Props = {
  messages: ConversationMessage[]
  hasMore: boolean
  isFetchingMore: boolean
  onLoadMore: () => void
  onRetryMessage: (failedAssistantMessageId: string) => void
}

function parseSystemPayload(content: string): { title: string; payload: unknown } {
  try {
    const parsed = JSON.parse(content) as { type?: string }
    if (parsed && parsed.type === 'agent_changed') {
      return { title: 'Agent changed', payload: parsed }
    }
    return { title: 'System event', payload: parsed }
  } catch {
    return { title: 'System message', payload: content }
  }
}

export function ChatThread({
  messages,
  hasMore,
  isFetchingMore,
  onLoadMore,
  onRetryMessage,
}: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const endRef = useRef<HTMLDivElement | null>(null)
  const [autoScrollEnabled, setAutoScrollEnabled] = useState(true)
  const [pendingPrepend, setPendingPrepend] = useState<null | {
    previousHeight: number
    previousCount: number
  }>(null)

  const marker = useMemo(() => {
    const last = messages[messages.length - 1]
    return `${messages.length}:${last?.id ?? ''}:${last?.content ?? ''}:${last?.status ?? ''}`
  }, [messages])

  useEffect(() => {
    setAutoScrollEnabled(true)
  }, [messages[0]?.conversation_id])

  useEffect(() => {
    const element = containerRef.current
    if (!element) return

    const onScroll = () => {
      const distanceFromBottom = element.scrollHeight - element.scrollTop - element.clientHeight
      setAutoScrollEnabled(distanceFromBottom < 96)
      if (element.scrollTop < 80 && hasMore && !isFetchingMore) {
        setPendingPrepend({ previousHeight: element.scrollHeight, previousCount: messages.length })
        onLoadMore()
      }
    }

    element.addEventListener('scroll', onScroll)
    onScroll()
    return () => element.removeEventListener('scroll', onScroll)
  }, [hasMore, isFetchingMore, messages.length, onLoadMore])

  useEffect(() => {
    if (!pendingPrepend) return
    if (messages.length <= pendingPrepend.previousCount) return
    const element = containerRef.current
    if (!element) return
    const delta = element.scrollHeight - pendingPrepend.previousHeight
    element.scrollTop += delta
    setPendingPrepend(null)
  }, [messages.length, pendingPrepend])

  useEffect(() => {
    if (!autoScrollEnabled) return
    endRef.current?.scrollIntoView({ block: 'end' })
  }, [autoScrollEnabled, marker])

  return (
    <div ref={containerRef} className="relative flex-1 space-y-3 overflow-y-auto p-4">
      <div className="flex items-center justify-center">
        {hasMore ? (
          <Button
            variant="outline"
            size="sm"
            className="gap-2"
            onClick={() => {
              const element = containerRef.current
              if (!element || isFetchingMore) return
              setPendingPrepend({
                previousHeight: element.scrollHeight,
                previousCount: messages.length,
              })
              onLoadMore()
            }}
            disabled={isFetchingMore}
          >
            <ArrowLineUp size={14} />
            {isFetchingMore ? 'Loading earlier…' : 'Load earlier messages'}
          </Button>
        ) : null}
      </div>

      {messages.map((message, index) => {
        if (message.role === 'user') {
          const entry: ChatUserEntry = {
            kind: 'user',
            sequence: message.sequence,
            timestamp: message.created_at,
            text: message.content,
          }
          return <ChatUserMessage key={message.id} entry={entry} />
        }

        if (message.role === 'system') {
          const { title, payload } = parseSystemPayload(message.content)
          const entry: ChatSystemEntry = {
            kind: 'system',
            sequence: message.sequence,
            timestamp: message.created_at,
            title,
            payload,
          }
          return <ChatSystemMessage key={message.id} entry={entry} />
        }

        const entry: ChatAssistantEntry = {
          kind: 'assistant',
          sequence: message.sequence,
          timestamp: message.created_at,
          text:
            message.content ||
            (message.status === 'streaming'
              ? '…'
              : message.status === 'cancelled'
                ? '(Stopped)'
                : ''),
          isStreaming: message.status === 'streaming',
        }
        const previousUser = [...messages.slice(0, index)]
          .reverse()
          .find((candidate) => candidate.role === 'user')

        return (
          <div key={message.id} className={cn('space-y-2')}>
            <ChatAssistantMessage entry={entry} />
            <div className="flex items-center gap-2 pl-2">
              {message.status === 'streaming' ? <Badge variant="outline">Streaming</Badge> : null}
              {message.status === 'cancelled' ? <Badge variant="outline">Stopped</Badge> : null}
              {message.status === 'failed' ? (
                <>
                  <Badge variant="destructive">Failed</Badge>
                  {message.error ? (
                    <span className="text-xs text-destructive">{message.error}</span>
                  ) : null}
                  <Button
                    size="sm"
                    variant="outline"
                    className="gap-1"
                    onClick={() => onRetryMessage(message.id)}
                    disabled={!previousUser}
                  >
                    <ArrowsClockwise size={13} />
                    Retry
                  </Button>
                </>
              ) : null}
            </div>
          </div>
        )
      })}

      <div ref={endRef} />

      {!autoScrollEnabled ? (
        <div className="sticky bottom-2 flex justify-center">
          <Button variant="outline" size="sm" onClick={() => setAutoScrollEnabled(true)}>
            Jump to latest
          </Button>
        </div>
      ) : null}
    </div>
  )
}
