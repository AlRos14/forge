import { useEffect, useMemo, useRef, useState } from 'react'
import { Archive, ArrowUp, CaretDown, GearSix, Plus, Robot, Square } from '@phosphor-icons/react'
import { Link, useNavigate } from '@tanstack/react-router'
import { toast } from 'sonner'
import {
  useAgentQuery,
  useAgentsQuery,
  useArchiveConversation,
  useConversationQuery,
  useConversationLogsQuery,
  useCancelConversationResponse,
  useConversationMessagesQuery,
  useConversationsQuery,
  useCreateConversation,
  useSendConversationMessage,
  useUpdateConversation,
} from '@/api/hooks'
import { ConversationViewer } from '@/components/conversation-viewer'
import { Avatar } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { InlineEdit } from '@/components/ui/inline-edit'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/cn'
import { getApiErrorMessage } from '@/lib/api-error'
import type { ConversationMessage, LogEntry } from '@/types/generated'

type Props = {
  projectId: string
  conversationId?: string
}

const SAMPLE_PROMPTS = [
  {
    title: 'Plan the next sprint',
    text: 'Help me plan the next sprint by breaking down our goals into actionable tasks.',
  },
  {
    title: 'Analyze blockers',
    text: "What's currently blocking progress in this project?",
  },
  {
    title: 'Draft a feature spec',
    text: 'Write a detailed specification for a new feature I want to build.',
  },
  {
    title: 'Review recent changes',
    text: "Summarize what's changed in this project recently and what still needs attention.",
  },
]

const CHAT_HIDDEN_LOG_KINDS: Array<LogEntry['kind'] | string> = [
  'session_info',
  'shell_command',
  'stdout',
  'stderr',
]

function toChronologicalMessages(
  data: ReturnType<typeof useConversationMessagesQuery>['data'],
): ConversationMessage[] {
  if (!data) return []
  return [...data.pages].reverse().flatMap((page) => page.items)
}

function relativeDate(value?: string | null): string {
  if (!value) return 'No activity'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'No activity'
  const diffMs = date.getTime() - Date.now()
  const absSec = Math.abs(Math.round(diffMs / 1000))
  if (absSec < 60) return 'just now'
  if (absSec < 3600) {
    const mins = Math.round(absSec / 60)
    return `${mins}m ${diffMs > 0 ? 'from now' : 'ago'}`
  }
  if (absSec < 86400) {
    const hours = Math.round(absSec / 3600)
    return `${hours}h ${diffMs > 0 ? 'from now' : 'ago'}`
  }
  const days = Math.round(absSec / 86400)
  return `${days}d ${diffMs > 0 ? 'from now' : 'ago'}`
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isLogKind(value: unknown): value is LogEntry['kind'] {
  return (
    value === 'stdout' ||
    value === 'stderr' ||
    value === 'tool_call' ||
    value === 'tool_result' ||
    value === 'assistant' ||
    value === 'assistant_delta' ||
    value === 'user' ||
    value === 'system' ||
    value === 'file_change' ||
    value === 'shell_command' ||
    value === 'approval_question' ||
    value === 'session_info' ||
    value === 'unknown'
  )
}

function parseLogEntry(value: unknown): LogEntry | undefined {
  if (!isRecord(value)) return undefined
  if (typeof value.sequence !== 'number') return undefined
  if (!isLogKind(value.kind)) return undefined
  if (typeof value.timestamp !== 'string') return undefined
  return {
    schema_version: typeof value.schema_version === 'number' ? value.schema_version : 1,
    sequence: value.sequence,
    timestamp: value.timestamp,
    execution_id: typeof value.execution_id === 'string' ? value.execution_id : '',
    kind: value.kind,
    stream: value.stream === 'heartbeat' ? 'heartbeat' : 'main',
    payload: value.payload,
    truncated: typeof value.truncated === 'boolean' ? value.truncated : false,
  }
}

function parseConversationLogEvent(detail: unknown, conversationId: string): LogEntry | undefined {
  if (!isRecord(detail)) return undefined
  if (detail.event_type !== 'conversation.log') return undefined
  if (detail.conversation_id !== conversationId && detail.entity_id !== conversationId) return undefined
  return parseLogEntry(detail.log)
}

function mergeLogs(existing: LogEntry[], incoming: LogEntry[]): LogEntry[] {
  const seen = new Set<string>()
  const merged: LogEntry[] = []
  for (const log of [...existing, ...incoming]) {
    const key = logIdentity(log)
    if (seen.has(key)) continue
    seen.add(key)
    merged.push(log)
  }
  return merged.sort(compareLogsChronologically)
}

function logIdentity(log: LogEntry): string {
  return `${log.execution_id}:${log.timestamp}:${log.kind}:${JSON.stringify(log.payload)}`
}

function compareLogsChronologically(a: LogEntry, b: LogEntry): number {
  const timestampOrder = Date.parse(a.timestamp) - Date.parse(b.timestamp)
  if (Number.isFinite(timestampOrder) && timestampOrder !== 0) return timestampOrder
  return a.sequence - b.sequence
}

export function ChatPage({ projectId, conversationId }: Props) {
  const navigate = useNavigate()
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const [search, setSearch] = useState('')
  const [draft, setDraft] = useState('')
  const [isNewChatDialogOpen, setIsNewChatDialogOpen] = useState(false)
  const [newTitle, setNewTitle] = useState('')
  const [newSystemPrompt, setNewSystemPrompt] = useState('')
  const [newConversationAgentId, setNewConversationAgentId] = useState('')
  const [isAgentPickerOpen, setIsAgentPickerOpen] = useState(false)
  const [pendingConversationAgentId, setPendingConversationAgentId] = useState('')
  const [pendingModel, setPendingModel] = useState('')
  const [pendingReasoning, setPendingReasoning] = useState('')
  const [quickChatAgentId, setQuickChatAgentId] = useState('')
  const [logs, setLogs] = useState<LogEntry[]>([])

  const agentsQuery = useAgentsQuery()
  const allAgents = agentsQuery.data?.items ?? []
  const agentOptions = allAgents.filter((agent) => agent.executor_type !== 'shell')
  const defaultAgentId = agentOptions[0]?.id

  const conversationsQuery = useConversationsQuery(projectId, 'active')
  const conversations = useMemo(() => {
    const query = search.trim().toLowerCase()
    return (
      conversationsQuery.data?.pages
        .flatMap((page) => page.items)
        .filter((conversation) => conversation.title.toLowerCase().includes(query)) ?? []
    )
  }, [conversationsQuery.data, search])

  const selectedConversationFromList = conversations.find(
    (conversation) => conversation.id === conversationId,
  )
  const selectedConversationQuery = useConversationQuery(conversationId ?? '')
  const selectedConversation = selectedConversationFromList ?? selectedConversationQuery.data
  const selectedAgentFromList = allAgents.find((agent) => agent.id === selectedConversation?.agent_id)
  const selectedAgentId = selectedConversation?.agent_id ?? undefined
  const selectedAgentQuery = useAgentQuery(selectedAgentFromList ? undefined : selectedAgentId)
  const selectedAgent = selectedAgentFromList ?? selectedAgentQuery.data

  const messagesQuery = useConversationMessagesQuery(conversationId ?? '')
  const messages = toChronologicalMessages(messagesQuery.data)
  const isStreaming = messages.some(
    (message) => message.role === 'assistant' && message.status === 'streaming',
  )
  const logsQuery = useConversationLogsQuery(conversationId ?? '', isStreaming)

  const createConversation = useCreateConversation(projectId)
  const archiveConversation = useArchiveConversation()
  const updateConversation = useUpdateConversation()
  const sendMessage = useSendConversationMessage()
  const cancelResponse = useCancelConversationResponse()

  useEffect(() => {
    if (!newConversationAgentId && defaultAgentId) {
      setNewConversationAgentId(defaultAgentId)
    }
  }, [defaultAgentId, newConversationAgentId])

  useEffect(() => {
    if (!quickChatAgentId && defaultAgentId) {
      setQuickChatAgentId(defaultAgentId)
    }
  }, [defaultAgentId, quickChatAgentId])

  useEffect(() => {
    setPendingConversationAgentId(selectedConversation?.agent_id ?? '')
    setIsAgentPickerOpen(false)
  }, [selectedConversation?.id, selectedConversation?.agent_id])

  useEffect(() => {
    setLogs([])
  }, [conversationId])

  useEffect(() => {
    if (!logsQuery.data) return
    setLogs((current) => mergeLogs(current, logsQuery.data.items))
  }, [logsQuery.data])

  useEffect(() => {
    if (!conversationId) return undefined

    const handleConversationLog = (event: Event) => {
      const log = parseConversationLogEvent(
        (event as CustomEvent<unknown>).detail,
        conversationId,
      )
      if (!log) return
      setLogs((current) => mergeLogs(current, [log]))
    }

    window.addEventListener('forge:conversation-log', handleConversationLog)
    return () => window.removeEventListener('forge:conversation-log', handleConversationLog)
  }, [conversationId])

  function adjustTextareaHeight() {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 192)}px`
  }

  async function handleCreateConversation() {
    if (!newConversationAgentId) return
    const created = await createConversation.mutateAsync({
      agent_id: newConversationAgentId,
      title: newTitle.trim() || undefined,
      system_prompt: newSystemPrompt.trim() || undefined,
    })
    setIsNewChatDialogOpen(false)
    setNewTitle('')
    setNewSystemPrompt('')
    setNewConversationAgentId(defaultAgentId ?? '')
    void navigate({
      to: '/projects/$projectId/chat/$conversationId',
      params: { projectId, conversationId: created.id },
    })
  }

  async function handleSend(contentOverride?: string) {
    if (!conversationId) return
    const content = (contentOverride ?? draft).trim()
    if (!content) return
    await sendMessage.mutateAsync({
      conversationId,
      body: {
        content,
        overrides: {
          model_id: pendingModel || undefined,
          reasoning_effort: pendingReasoning || undefined,
        },
      },
    })
    if (!contentOverride) setDraft('')
    setPendingModel('')
    setPendingReasoning('')
  }

  // Handles send from the input — auto-creates a conversation if none is selected
  async function handleQuickSend(content: string) {
    const trimmed = content.trim()
    if (!trimmed) return
    if (conversationId) {
      await handleSend(trimmed)
      setDraft('')
      const el = textareaRef.current
      if (el) el.style.height = 'auto'
      return
    }
    if (!quickChatAgentId) {
      toast.error('No agents available. Create an agent first.')
      return
    }
    setDraft('')
    const el = textareaRef.current
    if (el) el.style.height = 'auto'
    const overrides = {
      model_id: pendingModel || undefined,
      reasoning_effort: pendingReasoning || undefined,
    }
    const created = await createConversation.mutateAsync({ agent_id: quickChatAgentId })
    await sendMessage.mutateAsync({
      conversationId: created.id,
      body: {
        content: trimmed,
        overrides,
      },
    })
    setPendingModel('')
    setPendingReasoning('')
    void navigate({
      to: '/projects/$projectId/chat/$conversationId',
      params: { projectId, conversationId: created.id },
    })
  }

  async function handleChangeAgent() {
    if (!selectedConversation || !pendingConversationAgentId) return
    if (pendingConversationAgentId === selectedConversation.agent_id) {
      setIsAgentPickerOpen(false)
      return
    }
    await updateConversation.mutateAsync({
      conversationId: selectedConversation.id,
      body: {
        version: selectedConversation.version,
        agent_id: pendingConversationAgentId,
      },
    })
    setIsAgentPickerOpen(false)
  }

  const showWelcome = !selectedConversation || messages.length === 0

  return (
    <div className="-m-5 flex h-[calc(100%+2.5rem)] overflow-hidden">
      {/* Sidebar */}
      <aside className="flex w-64 shrink-0 flex-col border-r border-border-subtle bg-background">
        <div className="flex items-center gap-1.5 border-b border-border px-3 py-2.5">
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search conversations…"
            className="h-8 flex-1 text-xs"
          />
          <Button
            size="icon"
            variant="ghost"
            className="h-8 w-8 shrink-0"
            onClick={() =>
              void navigate({ to: '/projects/$projectId/chat', params: { projectId } })
            }
            title="New chat"
          >
            <Plus size={15} />
          </Button>
        </div>

        <div className="flex-1 overflow-y-auto px-2 py-2 space-y-0.5">
          {conversations.map((conversation) => {
            const rowAgent = allAgents.find((agent) => agent.id === conversation.agent_id)
            const isActive = conversation.id === conversationId
            return (
              <div
                key={conversation.id}
                className={cn(
                  'group relative flex items-start rounded-lg px-2 py-2 transition-colors',
                  isActive
                    ? 'bg-accent text-accent-foreground'
                    : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground',
                )}
              >
                <Link
                  to="/projects/$projectId/chat/$conversationId"
                  params={{ projectId, conversationId: conversation.id }}
                  className="flex min-w-0 flex-1 flex-col gap-0.5"
                >
                  <span className="block truncate text-sm font-medium text-foreground">
                    {conversation.title}
                  </span>
                  <span className="flex items-center justify-between gap-1 text-[11px]">
                    <span className="truncate">{rowAgent?.name ?? 'No agent'}</span>
                    <span className="shrink-0 text-muted-foreground">
                      {relativeDate(conversation.last_message_at)}
                    </span>
                  </span>
                </Link>
                <Button
                  size="icon"
                  variant="ghost"
                  className="ml-1 mt-0.5 h-6 w-6 shrink-0 rounded p-0.5 opacity-0 transition-opacity hover:bg-accent group-hover:opacity-100"
                  title="Archive conversation"
                  aria-label="Archive conversation"
                  onClick={() => void archiveConversation.mutateAsync(conversation.id)}
                >
                  <Archive size={14} weight="bold" />
                </Button>
              </div>
            )
          })}
          {conversations.length === 0 ? (
            <div className="px-2 py-8 text-center">
              <p className="mb-3 text-xs text-muted-foreground">No conversations yet</p>
              <Button
                size="sm"
                variant="outline"
                onClick={() => textareaRef.current?.focus()}
              >
                Start a Chat
              </Button>
            </div>
          ) : null}
        </div>
      </aside>

      {/* Main content */}
      <section className="relative flex min-w-0 flex-1 flex-col">
        {/* Header — only shown when a conversation is active */}
        {selectedConversation ? (
          <header className="flex items-center gap-2 border-b border-border px-4 py-2.5">
            <InlineEdit
              value={selectedConversation.title}
              onCommit={(next) => {
                void updateConversation.mutateAsync({
                  conversationId: selectedConversation.id,
                  body: { version: selectedConversation.version, title: next },
                }).catch((error) => {
                  toast.error(getApiErrorMessage(error, 'Title update failed'))
                })
              }}
            />

            <div className="flex shrink-0 items-center gap-1.5">
              {selectedAgent ? (
                <div className="flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs">
                  <Avatar name={selectedAgent.name} size="xs" />
                  <span className="font-medium">{selectedAgent.name}</span>
                  <Badge variant="outline" className="text-micro uppercase">
                    {selectedAgent.effective_status ?? selectedAgent.status}
                  </Badge>
                </div>
              ) : (
                <Badge variant="destructive" className="text-xs">No agent</Badge>
              )}
              <Button
                variant="ghost"
                size="sm"
                className="h-7 px-2 text-xs text-muted-foreground"
                onClick={() => setIsAgentPickerOpen((open) => !open)}
              >
                Change
              </Button>
              {isAgentPickerOpen ? (
                <div className="flex items-center gap-1.5">
                  <Select
                    value={pendingConversationAgentId}
                    placeholder="Select agent"
                    className="h-7 text-xs"
                    options={agentOptions.map((agent) => ({ value: agent.id, label: agent.name }))}
                    onChange={(v) => setPendingConversationAgentId(v)}
                  />
                  <Button
                    size="sm"
                    className="h-7 px-2 text-xs"
                    onClick={() => void handleChangeAgent()}
                    disabled={!pendingConversationAgentId || updateConversation.isPending}
                  >
                    Save
                  </Button>
                </div>
              ) : null}
            </div>
          </header>
        ) : null}

        {!selectedConversation && agentOptions.length > 0 ? (
          <div className="absolute left-4 top-4 z-10">
            <DropdownMenu>
              <DropdownMenuTrigger className="flex w-48 max-w-full items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-accent/60">
                <Avatar
                  name={agentOptions.find((a) => a.id === quickChatAgentId)?.name ?? ''}
                  size="xs"
                />
                <span
                  className="min-w-0 flex-1 truncate"
                  title={
                    agentOptions.find((a) => a.id === quickChatAgentId)?.name
                    ?? 'Select agent'
                  }
                >
                  {agentOptions.find((a) => a.id === quickChatAgentId)?.name ?? 'Select agent'}
                </span>
                <CaretDown size={12} className="shrink-0" />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="w-48">
                {agentOptions.map((agent) => (
                  <DropdownMenuItem
                    key={agent.id}
                    onClick={() => setQuickChatAgentId(agent.id)}
                  >
                    <Avatar name={agent.name} size="xs" />
                    <span className="ml-1.5 min-w-0 flex-1 truncate" title={agent.name}>
                      {agent.name}
                    </span>
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        ) : null}

        {/* Welcome / empty state */}
        {showWelcome ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-8 px-4 py-12">
            <div className="flex flex-col items-center gap-4 text-center">
              <div className="flex h-12 w-12 items-center justify-center rounded-2xl bg-foreground shadow-sm">
                <Robot size={22} className="text-background" weight="fill" />
              </div>
              <h2 className="text-2xl font-semibold tracking-tight">
                {selectedAgent ? `Chat with ${selectedAgent.name}` : 'What can I help with?'}
              </h2>
            </div>

            <div className="grid w-full max-w-2xl grid-cols-2 gap-3">
              {SAMPLE_PROMPTS.map((prompt) => (
                <button
                  key={prompt.title}
                  type="button"
                  onClick={() => {
                    setDraft(prompt.text)
                    setTimeout(adjustTextareaHeight, 0)
                    textareaRef.current?.focus()
                  }}
                  className="cursor-pointer rounded-xl border border-border p-4 text-left transition-colors hover:bg-accent/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <div className="text-sm font-medium">{prompt.title}</div>
                  <div className="mt-1 line-clamp-2 text-xs leading-relaxed text-muted-foreground">
                    {prompt.text}
                  </div>
                </button>
              ))}
            </div>
          </div>
        ) : (
          <ConversationViewer
            logs={logs}
            hiddenLogKinds={CHAT_HIDDEN_LOG_KINDS}
            autoScroll
            isLoadingHistory={logsQuery.isLoading}
            className="flex-1"
          />
        )}

        {/* Input footer */}
        <div className={cn('px-4 pb-3 pt-2', showWelcome && 'border-t-0')}>
          <div className="mx-auto max-w-3xl">
            {/* Override indicator */}
            {pendingModel || pendingReasoning ? (
              <div className="mb-2 flex items-center gap-2">
                <Badge variant="secondary" className="gap-1 text-[11px]">
                  <GearSix size={10} />
                  {pendingModel || 'default'}
                  {pendingReasoning ? ` · ${pendingReasoning}` : ''}
                </Badge>
                <button
                  type="button"
                  onClick={() => {
                    setPendingModel('')
                    setPendingReasoning('')
                  }}
                  className="cursor-pointer text-[11px] text-muted-foreground hover:text-foreground"
                >
                  Clear
                </button>
              </div>
            ) : null}

            {/* ChatGPT-style input */}
            <div className="relative rounded-2xl border border-border bg-background shadow-sm transition-colors focus-within:border-ring focus-within:ring-1 focus-within:ring-ring/30">
              <Textarea
                ref={textareaRef}
                value={draft}
                onChange={(event) => {
                  setDraft(event.target.value)
                  adjustTextareaHeight()
                }}
                placeholder={conversationId ? 'Ask anything…' : 'Start a new conversation…'}
                disabled={isStreaming}
                rows={1}
                className="min-h-[52px] max-h-48 resize-none rounded-2xl border-0 bg-transparent py-3.5 pl-4 pr-20 text-sm shadow-none focus-visible:outline-none focus-visible:ring-0 focus-visible:ring-offset-0"
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && !event.shiftKey) {
                    event.preventDefault()
                    void handleQuickSend(draft)
                  }
                }}
              />

              {/* Right-side controls inside input */}
              <div className="absolute bottom-2.5 right-2.5 flex items-center gap-1">
                <DropdownMenu>
                  <DropdownMenuTrigger
                    className={cn(
                      'flex h-7 w-7 items-center justify-center rounded-lg transition-colors hover:bg-accent',
                      (pendingModel || pendingReasoning) ? 'text-primary' : 'text-muted-foreground',
                    )}
                    title="Overrides"
                  >
                    <GearSix size={14} />
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" side="top" className="w-72 space-y-2 p-2">
                    <div className="space-y-1">
                      <Label htmlFor="chat-override-model" className="text-xs">Model</Label>
                      <Input
                        id="chat-override-model"
                        value={pendingModel}
                        onChange={(event) => setPendingModel(event.target.value)}
                        placeholder="e.g. claude-opus-4-6"
                        className="h-7 text-xs"
                      />
                    </div>
                    <div className="space-y-1">
                      <Label htmlFor="chat-override-reasoning" className="text-xs">Reasoning effort</Label>
                      <Select
                        id="chat-override-reasoning"
                        value={pendingReasoning}
                        placeholder="Default"
                        className="py-1 text-xs"
                        options={[
                          { value: 'low', label: 'Low' },
                          { value: 'medium', label: 'Medium' },
                          { value: 'high', label: 'High' },
                          { value: 'xhigh', label: 'XHigh' },
                        ]}
                        onChange={(v) => setPendingReasoning(v)}
                      />
                    </div>
                    <div className="flex justify-end pt-1">
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-7 text-xs"
                        onClick={() => {
                          setPendingModel('')
                          setPendingReasoning('')
                        }}
                      >
                        Clear
                      </Button>
                    </div>
                  </DropdownMenuContent>
                </DropdownMenu>

                {isStreaming ? (
                  <Button
                    size="icon"
                    variant="outline"
                    className="h-7 w-7 rounded-lg"
                    onClick={() =>
                      conversationId && void cancelResponse.mutateAsync(conversationId)
                    }
                    title="Stop generating"
                  >
                    <Square size={13} weight="fill" />
                  </Button>
                ) : (
                  <Button
                    size="icon"
                    className="h-7 w-7 rounded-lg"
                    onClick={() => void handleQuickSend(draft)}
                    disabled={!draft.trim() || sendMessage.isPending || createConversation.isPending}
                    title="Send"
                  >
                    <ArrowUp size={14} />
                  </Button>
                )}
              </div>
            </div>

          </div>
        </div>
      </section>

      {/* New chat dialog */}
      <Dialog open={isNewChatDialogOpen} onOpenChange={setIsNewChatDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New chat</DialogTitle>
            <DialogDescription>
              Choose an agent and optional prompt before starting.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <div className="space-y-1">
              <Label htmlFor="new-chat-agent">Agent</Label>
              <Select
                id="new-chat-agent"
                value={newConversationAgentId}
                placeholder="Select agent"
                options={agentOptions.map((agent) => ({ value: agent.id, label: agent.name }))}
                onChange={(v) => setNewConversationAgentId(v)}
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="new-chat-title">Title (optional)</Label>
              <Input
                id="new-chat-title"
                value={newTitle}
                onChange={(event) => setNewTitle(event.target.value)}
                placeholder="Project planning"
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="new-chat-system-prompt">System prompt (optional)</Label>
              <Textarea
                id="new-chat-system-prompt"
                value={newSystemPrompt}
                onChange={(event) => setNewSystemPrompt(event.target.value)}
                rows={4}
                placeholder="You are a PM assistant focused on roadmap and prioritization."
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setIsNewChatDialogOpen(false)}>
              Cancel
            </Button>
            <Button
              onClick={() => void handleCreateConversation()}
              disabled={!newConversationAgentId || createConversation.isPending}
            >
              Create chat
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
