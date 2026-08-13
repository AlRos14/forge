import { useEffect, useMemo } from 'react'
import { Link, useNavigate } from '@tanstack/react-router'
import { ArrowUpRight, ChatCircleDots } from '@phosphor-icons/react'
import { Avatar } from '@/components/ui/avatar'
import { ErrorPanel, LoadingPanel, StatusDot } from '@/features/federation/components'
import { AgentChatSwitcher } from '@/components/chat/agent-chat-switcher'
import type { ChatSwitcherEntry } from '@/components/chat/agent-chat-switcher'
import { AgentChatTimeline } from '@/components/chat/agent-chat-timeline'
import { ChatSetupRequired } from '@/components/chat/chat-setup-required'
import { ProductGenesisControls } from '@/features/product-genesis/ProductGenesisControls'
import {
  useAgentChatQuery,
  useAgentChatsQuery,
  useCancelAgentChatTurnMutation,
  useSendAgentChatMessageMutation,
} from '@/features/agent-chat/hooks'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import { useChatSelection } from '@/stores/chat'
import { useProjectsInfiniteQuery } from '@/api/hooks'

const PROJECTS_PAGE_SIZE = 100

function projectNameFor(
  projectId: string | null | undefined,
  projects: Array<{ id: string; name: string }>,
): string {
  if (!projectId) return 'Project'
  return projects.find((project) => project.id === projectId)?.name ?? 'Project'
}

function isBindingReady(entry: AgentChatEntry | undefined): boolean {
  return Boolean(entry && entry.binding_state === 'active' && entry.chat_status === 'ready')
}

function count(value: bigint | number): number {
  return typeof value === 'bigint' ? Number(value) : value
}

function toSwitcherEntry(
  entry: AgentChatEntry,
  projectName: string,
  active: boolean,
): ChatSwitcherEntry {
  const setupRequired = !isBindingReady(entry)
  return {
    id: entry.chat_id,
    label: entry.kind === 'main' ? 'Global · Main' : projectName,
    description:
      entry.kind === 'main' ? 'Account-owned Main Agent timeline' : 'Project-owned Agent timeline',
    agentName: entry.identity_name,
    agentStatus: setupRequired ? null : 'ready',
    setupRequired,
    pendingTurnCount: count(entry.pending_turn_count),
    active,
  }
}

function emptyGlobalEntry(active: boolean): ChatSwitcherEntry {
  return {
    id: 'main-agent-chat',
    label: 'Global · Main',
    description: 'Account-owned Main Agent timeline',
    agentName: null,
    agentStatus: null,
    setupRequired: true,
    active,
  }
}

export function handoffProjectIdsForScope(
  projectId: string | undefined,
  entries: AgentChatEntry[],
): string[] {
  if (projectId) return [projectId]
  return entries.flatMap((entry) => (entry.project_id ? [entry.project_id] : []))
}

function ScopeAffordances({ projectId, ready }: { projectId?: string; ready?: boolean }) {
  const affordances = projectId
    ? ['Project decisions', 'Commitments', 'Task management', 'Delivery outcomes', 'Provenance']
    : ['Discovery', 'Web research', 'Bounded portfolio summaries', 'Project handoffs']

  return (
    <section
      aria-label={projectId ? 'Project Agent capabilities' : 'Main Agent capabilities'}
      className="mt-4 flex min-w-0 flex-wrap items-center gap-1.5"
    >
      <span className="mr-1 text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
        {projectId ? 'Project scope' : 'Main scope'}
      </span>
      {affordances.map((affordance) => (
        <span
          key={affordance}
          className="rounded-full border border-border-subtle bg-muted/40 px-2 py-1 text-micro text-muted-foreground"
        >
          {affordance === 'Web research'
            ? `${affordance} · ${ready === undefined ? 'checking' : ready ? 'policy-controlled' : 'setup required'}`
            : affordance}
        </span>
      ))}
    </section>
  )
}

export function ChatPage({ projectId }: { projectId?: string }) {
  const navigate = useNavigate()
  const chatsQuery = useAgentChatsQuery()
  const projectsQuery = useProjectsInfiniteQuery(PROJECTS_PAGE_SIZE)
  const selectedGlobalChatId = useChatSelection((state) => state.globalChatId)
  const selectedProjectChatId = useChatSelection((state) =>
    projectId ? state.projectChatIds[projectId] : undefined,
  )
  const setGlobalChat = useChatSelection((state) => state.setGlobalChat)
  const setProjectChat = useChatSelection((state) => state.setProjectChat)
  const entries = chatsQuery.data?.items ?? []
  const projects = useMemo(
    () => projectsQuery.data?.pages.flatMap((page) => page.items) ?? [],
    [projectsQuery.data],
  )
  const globalSource = entries.find((entry) => entry.kind === 'main')
  const projectSources = entries.filter((entry) => entry.kind === 'project')
  const projectEntries = useMemo(() => {
    const mapped = projects.map((project) => {
      const source = projectSources.find((entry) => entry.project_id === project.id)
      if (source) {
        return toSwitcherEntry(
          source,
          project.name,
          Boolean(projectId && source.project_id === projectId),
        )
      }
      return {
        id: `project:${project.id}`,
        label: project.name,
        description: 'Project-owned Agent timeline',
        agentName: null,
        agentStatus: null,
        setupRequired: true,
        active: project.id === projectId,
      }
    })
    const knownProjects = new Set(projects.map((project) => project.id))
    return [
      ...mapped,
      ...projectSources
        .filter((entry) => entry.project_id && !knownProjects.has(entry.project_id))
        .map((entry) =>
          toSwitcherEntry(
            entry,
            entry.project_name ?? 'Project',
            Boolean(projectId && entry.project_id === projectId),
          ),
        ),
    ]
  }, [projectId, projectSources, projects])

  const globalEntry = globalSource
    ? toSwitcherEntry(globalSource, 'Global', !projectId)
    : emptyGlobalEntry(!projectId)
  const activeSource = projectId
    ? projectSources.find((entry) => entry.project_id === projectId)
    : globalSource
  const storedChatId = projectId ? selectedProjectChatId : selectedGlobalChatId
  const activeChatId = activeSource?.chat_id ?? (entries.length === 0 ? storedChatId : undefined)
  const chatQuery = useAgentChatQuery(activeChatId)
  const sendMutation = useSendAgentChatMessageMutation(chatQuery.data?.id)
  const cancelMutation = useCancelAgentChatTurnMutation(chatQuery.data?.id)
  const activeAgentName = activeSource?.identity_name ?? undefined
  const activeProjectName = projectNameFor(projectId, projects)
  const activeReady = Boolean(
    chatQuery.data &&
    chatQuery.data.status === 'ready' &&
    activeSource &&
    isBindingReady(activeSource),
  )
  const activeStatusKnown = Boolean(activeSource || chatQuery.data)
  const chatNeedsSetup = Boolean(
    activeSource?.binding_state === 'setup_required' || chatQuery.data?.status === 'setup_required',
  )

  useEffect(() => {
    if (!chatQuery.data) return
    if (projectId) setProjectChat(projectId, chatQuery.data)
    else setGlobalChat(chatQuery.data)
  }, [chatQuery.data, projectId, setGlobalChat, setProjectChat])

  async function sendMessage(content: string) {
    if (!chatQuery.data) throw new Error('This Agent Chat is not ready yet.')
    const admitted = await sendMutation.mutateAsync({ content, dedupe_key: null })
    if (admitted.turn_job) {
      useChatSelection.getState().setPendingTurns(chatQuery.data.id, [admitted.turn_job])
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

  const pageTitle = projectId ? `${activeProjectName} chat` : 'Global chat'
  const pageDescription = projectId
    ? 'Continue with this Project’s single Agent Chat. Its timeline and context remain scoped here.'
    : 'The account’s single Main Agent timeline for discovery, organization, and explicit handoff.'

  return (
    <div className="flex min-h-0 min-h-full flex-col overflow-hidden rounded-xl border border-border-subtle bg-background shadow-xs">
      <header className="flex shrink-0 flex-wrap items-start justify-between gap-4 border-b border-border-subtle px-4 py-4 sm:px-5">
        <div className="min-w-0">
          <p className="font-mono text-micro uppercase tracking-[0.18em] text-primary">
            Agent Chat
          </p>
          <h1 className="mt-1 truncate text-xl font-semibold tracking-tight text-foreground sm:text-2xl">
            {pageTitle}
          </h1>
          <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
            {pageDescription}
          </p>
          <ScopeAffordances
            projectId={projectId}
            ready={activeStatusKnown ? activeReady : undefined}
          />
        </div>
        <div className="flex shrink-0 items-center gap-2 rounded-lg border border-border-subtle bg-muted/20 px-3 py-2">
          <ChatCircleDots size={15} className="text-primary" aria-hidden />
          <span className="text-xs text-muted-foreground">One durable timeline</span>
        </div>
        {!projectId ? <ProductGenesisControls /> : null}
      </header>
      <div className="flex min-h-0 flex-1 flex-col lg:flex-row">
        <AgentChatSwitcher
          globalEntry={globalEntry}
          projectEntries={projectEntries}
          onSelectGlobal={() => void navigate({ to: '/chat' })}
          onSelectProject={(entry) => {
            const source = projectSources.find((candidate) => candidate.chat_id === entry.id)
            const targetProjectId = source?.project_id ?? entry.id.replace(/^project:/, '')
            void navigate({
              to: '/projects/$projectId/chat',
              params: { projectId: targetProjectId },
            })
          }}
        />
        <main className="flex min-h-0 min-w-0 flex-1 flex-col">
          <header className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-border-subtle px-4 py-3 sm:px-5">
            <div className="flex min-w-0 items-center gap-3">
              <Avatar
                name={activeAgentName ?? (projectId ? activeProjectName : 'Main')}
                seed={activeChatId ?? projectId ?? 'main'}
                size="sm"
              />
              <div className="min-w-0">
                <p className="truncate text-sm font-semibold text-foreground">
                  {activeAgentName ?? (projectId ? 'Project Agent' : 'Main Agent')}
                </p>
                <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <StatusDot
                    status={chatQuery.data?.status === 'ready' ? 'ready' : 'setup_required'}
                  />
                  {chatQuery.data?.status === 'ready'
                    ? 'Ready for one finite turn'
                    : 'Setup required'}
                </p>
              </div>
            </div>
            <div className="flex flex-wrap items-center justify-end gap-3">
              {projectId ? (
                <>
                  <Link
                    to="/projects/$projectId/tasks"
                    params={{ projectId }}
                    search={{ sort_by: 'updated_at', sort_order: 'desc' }}
                    className="inline-flex items-center gap-1 text-xs font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    Open Project Tasks
                    <ArrowUpRight size={13} aria-hidden />
                  </Link>
                  <Link
                    to="/projects/$projectId/settings/$tab"
                    params={{ projectId, tab: 'project-agent' }}
                    className="inline-flex items-center gap-1 text-xs font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    Agent settings
                    <ArrowUpRight size={13} aria-hidden />
                  </Link>
                </>
              ) : null}
            </div>
          </header>
          {chatsQuery.isLoading || (activeChatId && chatQuery.isLoading) ? (
            <LoadingPanel label="Loading Agent Chat" />
          ) : chatsQuery.isError ? (
            <ErrorPanel
              title="Agent Chat unavailable"
              description="Forge could not load the authorized chat switcher. No chat is created or forked while it is unavailable."
              onRetry={() => void chatsQuery.refetch()}
            />
          ) : chatQuery.isError ? (
            <ErrorPanel
              title="Chat details unavailable"
              description="Forge could not load this existing Agent Chat. Try again before admitting a turn."
              onRetry={() => void chatQuery.refetch()}
            />
          ) : chatNeedsSetup ? (
            <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-4 sm:p-6">
              <ChatSetupRequired projectId={projectId} />
            </div>
          ) : chatQuery.data ? (
            <AgentChatTimeline
              chat={chatQuery.data}
              agentName={activeAgentName}
              projectId={projectId}
              handoffProjectIds={handoffProjectIdsForScope(projectId, projectSources)}
              isSending={sendMutation.isPending}
              onSend={sendMessage}
              onCancelTurn={cancelTurn}
            />
          ) : (
            <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-4 sm:p-6">
              <ChatSetupRequired projectId={projectId} />
            </div>
          )}
        </main>
      </div>
    </div>
  )
}
