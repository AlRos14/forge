import { useCallback, useEffect, useRef, useState } from 'react'
import { Link } from '@tanstack/react-router'
import { ChatCircleDots, X } from '@phosphor-icons/react'
import { AgentChatTimeline } from '@/components/chat/agent-chat-timeline'
import { ChatSetupRequired } from '@/components/chat/chat-setup-required'
import { ProductGenesisControls } from '@/features/product-genesis/ProductGenesisControls'
import { ErrorPanel, LoadingPanel } from '@/features/federation/components'
import {
  useAgentChatQuery,
  useAgentChatsQuery,
  useCancelAgentChatTurnMutation,
  useSendAgentChatMessageMutation,
} from '@/features/agent-chat/hooks'
import { useChatSelection } from '@/stores/chat'

export function ChatLauncher() {
  const [open, setOpen] = useState(false)
  const launcherRef = useRef<HTMLButtonElement>(null)
  const panelHeadingRef = useRef<HTMLHeadingElement>(null)
  const chatsQuery = useAgentChatsQuery()
  const globalEntry = chatsQuery.data?.items.find((entry) => entry.kind === 'main')
  const handoffProjectIds =
    chatsQuery.data?.items.flatMap((entry) =>
      entry.kind === 'project' && entry.project_id ? [entry.project_id] : [],
    ) ?? []
  // The switcher response is the server-authoritative owner of the singular
  // Main timeline. A local selection can be stale or Project-owned, so it is
  // never used to recover a missing Main entry in this global surface.
  const globalChatId = globalEntry?.chat_id
  const globalChatQuery = useAgentChatQuery(globalChatId)
  const sendMutation = useSendAgentChatMessageMutation(globalChatQuery.data?.id)
  const cancelMutation = useCancelAgentChatTurnMutation(globalChatQuery.data?.id)
  const setGlobalChat = useChatSelection((state) => state.setGlobalChat)

  const close = useCallback(() => {
    setOpen(false)
    requestAnimationFrame(() => launcherRef.current?.focus())
  }, [])

  useEffect(() => {
    if (!open) return
    panelHeadingRef.current?.focus()
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close()
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [close, open])

  useEffect(() => {
    if (globalChatQuery.data) {
      setGlobalChat(globalChatQuery.data)
    }
  }, [globalChatQuery.data, setGlobalChat])

  async function sendMessage(content: string) {
    if (!globalChatQuery.data) throw new Error('The global Main Agent Chat is not ready yet.')
    const admitted = await sendMutation.mutateAsync({ content, dedupe_key: null })
    if (admitted.turn_job) {
      useChatSelection.getState().setPendingTurns(globalChatQuery.data.id, [admitted.turn_job])
    }
  }

  async function cancelTurn(turnId: string, expectedVersion: number) {
    await cancelMutation.mutateAsync({
      turnId,
      input: {
        expected_version: expectedVersion,
        idempotency_key: `agent-chat-turn-cancel:${turnId}:${expectedVersion}`,
      },
    })
  }

  return (
    <div className="fixed bottom-4 right-4 z-30 flex flex-col items-end gap-3 sm:bottom-5 sm:right-5">
      {open ? (
        <section
          id="global-chat-launcher-panel"
          role="dialog"
          aria-modal="false"
          aria-labelledby="global-chat-launcher-heading"
          className="flex h-[min(38rem,calc(100dvh-6rem))] w-[min(28rem,calc(100vw-2rem))] min-w-0 flex-col overflow-hidden rounded-xl border border-border-subtle bg-background shadow-float"
        >
          <header className="flex shrink-0 items-start justify-between gap-3 border-b border-border-subtle px-4 py-3">
            <div className="min-w-0">
              <h2
                id="global-chat-launcher-heading"
                ref={panelHeadingRef}
                tabIndex={-1}
                className="text-sm font-semibold text-foreground focus-visible:outline-none"
              >
                Global · Main
              </h2>
              <p className="mt-0.5 text-xs text-muted-foreground">
                The same account timeline as full chat
              </p>
            </div>
            <ProductGenesisControls />
            <button
              type="button"
              onClick={close}
              aria-label="Close global chat"
              className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <X size={15} aria-hidden />
            </button>
          </header>
          <div className="flex min-h-0 flex-1 flex-col">
            {chatsQuery.isLoading || (globalChatId && globalChatQuery.isLoading) ? (
              <LoadingPanel label="Loading global chat" />
            ) : chatsQuery.isError ? (
              <ErrorPanel
                title="Global chat unavailable"
                description="Forge could not load the global Main Agent Chat right now."
                onRetry={() => void chatsQuery.refetch()}
              />
            ) : globalChatQuery.isError ? (
              <ErrorPanel
                title="Global timeline unavailable"
                description="Forge could not load the existing Main Agent timeline."
                onRetry={() => void globalChatQuery.refetch()}
              />
            ) : globalChatQuery.data ? (
              <AgentChatTimeline
                chat={globalChatQuery.data}
                agentName={globalEntry?.identity_name ?? undefined}
                handoffProjectIds={handoffProjectIds}
                isSending={sendMutation.isPending}
                onSend={sendMessage}
                onCancelTurn={cancelTurn}
              />
            ) : (
              <div className="min-h-0 flex-1 overflow-y-auto p-4">
                <ChatSetupRequired />
              </div>
            )}
          </div>
          <footer className="shrink-0 border-t border-border-subtle px-4 py-2 text-right">
            <Link
              to="/chat"
              onClick={() => close()}
              className="text-xs font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              Open full global chat
            </Link>
          </footer>
        </section>
      ) : null}
      <button
        ref={launcherRef}
        type="button"
        aria-expanded={open}
        aria-controls="global-chat-launcher-panel"
        aria-label={open ? 'Close global chat' : 'Open global chat'}
        className="flex h-12 w-12 items-center justify-center rounded-full border border-ember-border bg-primary text-primary-foreground shadow-float transition-transform hover:scale-[1.03] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        onClick={() => setOpen((current) => !current)}
      >
        {open ? <X size={20} aria-hidden /> : <ChatCircleDots size={20} aria-hidden />}
      </button>
    </div>
  )
}
