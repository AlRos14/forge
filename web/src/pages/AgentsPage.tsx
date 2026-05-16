import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { Plus, Robot } from '@phosphor-icons/react'
import { toast } from 'sonner'
import {
  useAgentQuery,
  useAgentsInfiniteQuery,
  useCreateAgent,
  useDaemonsQuery,
  useDeleteAgent,
  useDuplicateAgent,
  usePauseAgent,
  useResumeAgent,
  useUpdateAgent,
} from '@/api/hooks'
import { ErrorBanner } from '@/components/error-banner'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { getApiErrorMessage, isApiStatus } from '@/lib/api-error'
import { useAuthStore } from '@/stores/auth'
import type { UpdateAgentRequest } from '@/types/generated'
import { AgentList } from './agents/AgentList'
import { CreatePanel } from './agents/CreatePanel'
import { EditPanel } from './agents/EditPanel'
import { EmptyPanel } from './agents/EmptyPanel'
import { ViewPanel } from './agents/ViewPanel'
import { AGENTS_PAGE_SIZE, EMPTY_AGENTS, EMPTY_DAEMONS } from './agents/constants'
import { agentToForm, emptyForm, parseForm, type AgentFormState } from './agents/form-utils'

type PanelMode = 'view' | 'edit'

export function AgentsPage({
  selectedAgentId,
  mode = 'view',
}: {
  selectedAgentId?: string
  mode?: 'view' | 'create' | 'edit'
}) {
  const navigate = useNavigate()
  const isAdmin = useAuthStore((s) => Boolean(s.user?.is_admin))
  const agentsQuery = useAgentsInfiniteQuery(AGENTS_PAGE_SIZE)
  const daemonsQuery = useDaemonsQuery(isAdmin)
  const createAgent = useCreateAgent()
  const updateAgent = useUpdateAgent()
  const deleteAgent = useDeleteAgent()
  const duplicateAgent = useDuplicateAgent()
  const pauseAgent = usePauseAgent()
  const resumeAgent = useResumeAgent()

  const [panelMode, setPanelMode] = useState<PanelMode>('view')
  const [confirmingDelete, setConfirmingDelete] = useState(false)
  const [form, setForm] = useState<AgentFormState>(emptyForm)
  const initializedEditAgentIdRef = useRef<string | null>(null)
  const agentsScrollRef = useRef<HTMLDivElement | null>(null)
  const agentsLoadMoreRef = useRef<HTMLDivElement | null>(null)

  const agentQuery = useAgentQuery(selectedAgentId)
  const selectedAgent = agentQuery.data
  const agents = useMemo(
    () => agentsQuery.data?.pages.flatMap((page) => page.items) ?? EMPTY_AGENTS,
    [agentsQuery.data],
  )
  const totalAgents = agentsQuery.data?.pages[0]?.total_count ?? null
  const daemons = daemonsQuery.data?.items ?? EMPTY_DAEMONS
  const isCreating = mode === 'create'
  const isEditingRoute = mode === 'edit'

  useEffect(() => {
    setPanelMode(isEditingRoute ? 'edit' : 'view')
    setConfirmingDelete(false)
    initializedEditAgentIdRef.current = null
  }, [isCreating, isEditingRoute, selectedAgentId])

  useEffect(() => {
    if (!isEditingRoute || !selectedAgent) return
    if (initializedEditAgentIdRef.current === selectedAgent.id) return
    setForm(agentToForm(selectedAgent))
    initializedEditAgentIdRef.current = selectedAgent.id
  }, [isEditingRoute, selectedAgent])

  useEffect(() => {
    const root = agentsScrollRef.current
    const node = agentsLoadMoreRef.current
    if (!root || !node || !agentsQuery.hasNextPage) return

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting && !agentsQuery.isFetchingNextPage) {
          void agentsQuery.fetchNextPage()
        }
      },
      { root, rootMargin: '160px 0px' },
    )

    observer.observe(node)
    return () => observer.disconnect()
  }, [
    agents.length,
    agentsQuery.fetchNextPage,
    agentsQuery.hasNextPage,
    agentsQuery.isFetchingNextPage,
  ])

  const selectAgent = (agentId: string) => {
    void navigate({ to: '/agents/$agentId', params: { agentId } })
  }

  const startCreate = () => {
    setForm(emptyForm)
    void navigate({ to: '/agents/new' })
  }

  const startEdit = () => {
    if (!selectedAgent) return
    setForm(agentToForm(selectedAgent))
    initializedEditAgentIdRef.current = selectedAgent.id
    void navigate({ to: '/agents/$agentId/edit', params: { agentId: selectedAgent.id } })
  }

  const cancelEdit = () => {
    setPanelMode('view')
    setConfirmingDelete(false)
    if (isCreating) {
      void navigate({ to: '/agents' })
    } else if (isEditingRoute && selectedAgentId) {
      void navigate({ to: '/agents/$agentId', params: { agentId: selectedAgentId } })
    }
  }

  const submitCreate = () => {
    try {
      const parsed = parseForm(form, { includeDaemon: isAdmin })
      createAgent.mutate(
        { name: form.name.trim(), executor_type: form.executor_type, ...parsed },
        {
          onSuccess: (agent) => {
            setForm(emptyForm)
            void navigate({ to: '/agents/$agentId', params: { agentId: agent.id } })
          },
          onError: (error) => toast.error(getApiErrorMessage(error, 'Agent creation failed')),
        },
      )
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'Invalid form')
    }
  }

  const submitEdit = () => {
    if (!selectedAgent) return
    try {
      const parsed = parseForm(form, { includeDaemon: isAdmin })
      const body: UpdateAgentRequest = {
        ...parsed,
        name: form.name.trim(),
        version: selectedAgent.version,
      }
      updateAgent.mutate(
        { agentId: selectedAgent.id, body },
        {
          onSuccess: () => {
            setPanelMode('view')
            void navigate({ to: '/agents/$agentId', params: { agentId: selectedAgent.id } })
          },
          onError: (error) => {
            if (isApiStatus(error, 409)) toast.error('Version conflict — reload and retry')
            else toast.error(getApiErrorMessage(error, 'Agent update failed'))
          },
        },
      )
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'Invalid form')
    }
  }

  const submitDelete = () => {
    if (!selectedAgent || deleteAgent.isPending) return
    if ((selectedAgent.active_task_count ?? 0) > 0) {
      toast.error('Cannot delete an agent with active tasks')
      return
    }
    deleteAgent.mutate(selectedAgent.id, {
      onSuccess: () => {
        setPanelMode('view')
        setConfirmingDelete(false)
        void navigate({ to: '/agents' })
      },
      onError: (error) => toast.error(getApiErrorMessage(error, 'Agent deletion failed')),
    })
  }

  const submitDuplicate = () => {
    if (!selectedAgent) return
    duplicateAgent.mutate(
      { agentId: selectedAgent.id, name: `${selectedAgent.name} (copy)` },
      {
        onSuccess: (agent) => {
          setPanelMode('view')
          void navigate({ to: '/agents/$agentId', params: { agentId: agent.id } })
        },
        onError: (error) => toast.error(getApiErrorMessage(error, 'Duplicate failed')),
      },
    )
  }

  const toggleAgentPaused = () => {
    if (!selectedAgent) return
    const mutation = selectedAgent.paused ? resumeAgent : pauseAgent
    mutation.mutate(selectedAgent.id, {
      onError: (error) =>
        toast.error(
          getApiErrorMessage(error, selectedAgent.paused ? 'Agent resume failed' : 'Agent pause failed'),
        ),
    })
  }

  const canDelete = selectedAgent && (selectedAgent.active_task_count ?? 0) === 0

  return (
    <div className="flex h-[calc(100vh-7rem)] gap-0 overflow-hidden rounded-xl border border-border-subtle bg-card shadow-card">
      <div className="flex w-60 shrink-0 flex-col border-r border-border-subtle bg-background">
        <header className="flex shrink-0 items-center justify-between border-b px-4 py-3">
          <div>
            <p className="font-mono text-micro font-semibold uppercase tracking-[1px] text-muted-foreground">Agents</p>
            <p className="mt-0.5 text-[11px] text-muted-foreground">
              {agents.filter((a) => a.status === 'idle' || a.status === 'busy').length}/{agents.length} active
              {totalAgents && totalAgents > agents.length ? ` · ${agents.length}/${totalAgents} loaded` : ''}
            </p>
          </div>
          <button
            type="button"
            className="flex h-6 w-6 cursor-pointer items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            onClick={startCreate}
            title="New agent"
          >
            <Plus size={14} weight="bold" />
          </button>
        </header>

        <div ref={agentsScrollRef} className="flex-1 overflow-y-auto">
          {agentsQuery.isError ? (
            <div className="p-3">
              <ErrorBanner
                error={agentsQuery.error}
                fallback="Failed to load agents"
                onRetry={() => void agentsQuery.refetch()}
              />
            </div>
          ) : agentsQuery.isLoading ? (
            <div className="space-y-1 p-2">
              {[0, 1, 2].map((i) => (
                <div key={i} className="rounded-lg p-3">
                  <Skeleton className="h-4 w-2/3" />
                  <Skeleton className="mt-2 h-3 w-full" />
                </div>
              ))}
            </div>
          ) : agents.length === 0 ? (
            <div className="p-6 text-center">
              <div className="mx-auto mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-muted">
                <Robot size={20} className="text-muted-foreground" />
              </div>
              <p className="text-sm font-medium">No agents</p>
              <p className="mt-1 text-xs text-muted-foreground">Create your first agent</p>
              <Button className="mt-3" size="sm" onClick={startCreate}>
                New Agent
              </Button>
            </div>
          ) : (
            <>
              <AgentList
                agents={agents}
                selectedId={!isCreating ? selectedAgentId : undefined}
                onSelect={selectAgent}
              />
              {agentsQuery.hasNextPage || agentsQuery.isFetchingNextPage ? (
                <div ref={agentsLoadMoreRef} className="px-3 pb-3 pt-1 text-center">
                  <Button
                    size="sm"
                    variant="ghost"
                    className="w-full text-xs text-muted-foreground"
                    disabled={agentsQuery.isFetchingNextPage}
                    onClick={() => void agentsQuery.fetchNextPage()}
                  >
                    {agentsQuery.isFetchingNextPage ? 'Loading...' : 'Load more'}
                  </Button>
                </div>
              ) : null}
            </>
          )}
        </div>
      </div>

      <div className="flex flex-1 flex-col overflow-hidden">
        {isCreating ? (
          <CreatePanel
            form={form}
            daemons={daemons}
            showDaemonSelector={isAdmin}
            pending={createAgent.isPending}
            onUpdate={setForm}
            onSubmit={submitCreate}
            onCancel={cancelEdit}
          />
        ) : selectedAgentId && selectedAgent ? (
          panelMode === 'edit' ? (
            <EditPanel
              agent={selectedAgent}
              form={form}
              daemons={daemons}
              showDaemonSelector={isAdmin}
              pending={updateAgent.isPending}
              canDelete={Boolean(canDelete)}
              deletePending={deleteAgent.isPending}
              confirmingDelete={confirmingDelete}
              onUpdate={setForm}
              onSubmit={submitEdit}
              onCancel={cancelEdit}
              onDelete={submitDelete}
              onConfirmDelete={() => setConfirmingDelete(true)}
              onCancelDelete={() => setConfirmingDelete(false)}
            />
          ) : (
            <ViewPanel
              agent={selectedAgent}
              daemons={daemons}
              showDaemonDetails={isAdmin}
              onEdit={startEdit}
              onDuplicate={submitDuplicate}
              onTogglePaused={toggleAgentPaused}
              duplicatePending={duplicateAgent.isPending}
              pausePending={pauseAgent.isPending || resumeAgent.isPending}
            />
          )
        ) : selectedAgentId && agentQuery.isLoading ? (
          <div className="flex-1 p-6">
            <Skeleton className="h-8 w-1/3" />
            <Skeleton className="mt-3 h-4 w-2/3" />
            <Skeleton className="mt-6 h-32 w-full" />
          </div>
        ) : (
          <EmptyPanel onCreate={startCreate} />
        )}
      </div>
    </div>
  )
}
