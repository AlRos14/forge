import type { MouseEvent } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { DragDropContext, type DragStart, type DragUpdate, type DropResult } from '@hello-pangea/dnd'
import { Funnel, Kanban, MagnifyingGlass, Plus, X } from '@phosphor-icons/react'
import { useNavigate, useSearch } from '@tanstack/react-router'
import { toast } from 'sonner'
import { ApiError } from '@/api/client'
import {
  useAgentsQuery,
  useAssignRole,
  useCreateTask,
  useReorderTask,
  useTasksQuery,
  useTransitionTask,
  useWorkflowQuery,
} from '@/api/hooks'
import { AgentFilterGroup } from '@/components/agent-filter-group'
import { ErrorBanner } from '@/components/error-banner'
import { TaskCreateDialog } from '@/components/task-create-dialog'
import { DropdownMenuItem } from '@/components/ui/dropdown-menu'
import { TaskDetailModal } from '@/components/task-detail-modal'
import { Button } from '@/components/ui/button'
import { taskStatusTransitions } from '@/components/task-controls'
import { toastApiError } from '@/lib/api-error'
import { cn } from '@/lib/cn'
import {
  type ColumnGroup,
  deriveColumns,
  getValidDropColumns,
  groupByColumns,
  matchesFilters,
  patchTaskIntoStatus,
} from '@/lib/workflow-utils'
import { useFilterStore } from '@/stores/filters'
import type { PositionRequest, Task, WorkflowDefinition } from '@/types/generated'
import { KanbanColumn } from './kanban-column'
import type { TaskCardMenuRenderer } from './kanban-task-card'

const DEFAULT_COLUMNS: ColumnGroup[] = [
  {
    primaryState: 'todo',
    columnName: 'Todo',
    states: ['todo'],
    stateLabels: { todo: 'Todo' },
    subStates: [],
    taskCount: 0,
    isTerminal: false,
    kind: 'initial',
    dotColor: 'bg-stone-500',
    accentColor: 'border-l-stone-500',
  },
  {
    primaryState: 'in_progress',
    columnName: 'In Progress',
    states: ['in_progress'],
    stateLabels: { in_progress: 'In Progress' },
    subStates: [],
    taskCount: 0,
    isTerminal: false,
    kind: 'active',
    dotColor: 'bg-orange-500',
    accentColor: 'border-l-orange-500',
  },
  {
    primaryState: 'review',
    columnName: 'Review',
    states: ['review'],
    stateLabels: { review: 'Review' },
    subStates: [],
    taskCount: 0,
    isTerminal: false,
    kind: 'gate',
    dotColor: 'bg-violet-400',
    accentColor: 'border-l-violet-400',
  },
  {
    primaryState: 'done',
    columnName: 'Done',
    states: ['done'],
    stateLabels: { done: 'Done' },
    subStates: [],
    taskCount: 0,
    isTerminal: true,
    kind: 'terminal',
    dotColor: 'bg-stone-500',
    accentColor: 'border-l-stone-500',
  },
]

type BoardSearch = {
  agentIds?: string
  priorityMax?: number
  priorityMin?: number
  q?: string
  task?: string
  blockedOnly?: boolean
  includeCancelled?: boolean
  includeArchived?: boolean
}

type BoardFilterPatch = Omit<Partial<BoardSearch>, 'agentIds'> & {
  agentIds?: string[]
}

export function KanbanBoard({ projectId }: { projectId: string }) {
  const navigate = useNavigate({ from: '/projects/$projectId/board' })
  const search = useSearch({ from: '/projects/$projectId/board' }) as BoardSearch
  const filterQ = useFilterStore((s) => s.q)
  const filterAgentIds = useFilterStore((s) => s.agentIds)
  const filterPriorityMin = useFilterStore((s) => s.priorityMin)
  const filterPriorityMax = useFilterStore((s) => s.priorityMax)
  const filterBlockedOnly = useFilterStore((s) => s.blockedOnly)
  const filterIncludeCancelled = useFilterStore((s) => s.includeCancelled)
  const filterIncludeArchived = useFilterStore((s) => s.includeArchived)
  const setFilters = useFilterStore((s) => s.setFilters)
  const [tasks, setTasks] = useState<Task[]>([])
  const [pendingReorderIds, setPendingReorderIds] = useState<Set<string>>(() => new Set())
  const [draggingTaskId, setDraggingTaskId] = useState<string>()
  const [validDropStatuses, setValidDropStatuses] = useState<string[]>([])
  const [activeDropStatus, setActiveDropStatus] = useState<string>()
  const [contextMenu, setContextMenu] = useState<{ task: Task; x: number; y: number }>()
  const [agentPickerTaskId, setAgentPickerTaskId] = useState<string>()
  const [quickCreateOpen, setQuickCreateOpen] = useState(false)
  const [quickCreateTitle, setQuickCreateTitle] = useState('')
  const [quickCreateDescription, setQuickCreateDescription] = useState('')
  const [createDialogOpen, setCreateDialogOpen] = useState(false)
  const [selectedTaskId, setSelectedTaskId] = useState<string>()
  const [showMobileFilters, setShowMobileFilters] = useState(false)
  const lastSyncedAllTasksRef = useRef<Task[] | undefined>(undefined)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const quickCreateDescriptionRef = useRef<HTMLTextAreaElement>(null)
  const agentUuids = filterAgentIds.filter((id) => id !== 'user')
  const tasksQuery = useTasksQuery(projectId, {
    q: filterQ || undefined,
    agent_id: agentUuids.length > 0 ? agentUuids.join(',') : undefined,
    assignee_type: filterAgentIds.includes('user') ? 'user' : undefined,
    include_cancelled: filterIncludeCancelled || undefined,
    include_archived: filterIncludeArchived || undefined,
    limit: 200,
  })

  const handleLoadMore = () => {
    if (tasksQuery.hasNextPage && !tasksQuery.isFetchingNextPage) {
      void tasksQuery.fetchNextPage()
    }
  }
  const workflowQuery = useWorkflowQuery(projectId)
  const agentsQuery = useAgentsQuery()
  const createTask = useCreateTask(projectId)
  const transitionTask = useTransitionTask()
  const reorderTask = useReorderTask()
  const assignRole = useAssignRole()
  const agentNamesById = useMemo(
    () => new Map((agentsQuery.data?.items ?? []).map((agent) => [agent.id, agent.name])),
    [agentsQuery.data],
  )

  const boardColumns = useMemo<ColumnGroup[]>(() => {
    const cols = workflowQuery.data ? deriveColumns(workflowQuery.data, tasks) : DEFAULT_COLUMNS
    const visible = cols.filter((col) => {
      if (['merging', 'merge_failed'].includes(col.primaryState)) return false
      if (col.primaryState === 'cancelled' && !filterIncludeCancelled) return false
      return true
    })
    if (filterIncludeCancelled && !visible.some((c) => c.primaryState === 'cancelled')) {
      // Strip 'cancelled' from any column that absorbed it (e.g. Done), since we show it separately
      const stripped = visible.map((col) => {
        if (!col.states.includes('cancelled')) return col
        const states = col.states.filter((s) => s !== 'cancelled')
        return {
          ...col,
          states,
          taskCount: tasks.filter((t) => states.includes(t.status)).length,
        }
      })
      stripped.push({
        primaryState: 'cancelled',
        columnName: 'Cancelled',
        states: ['cancelled'],
        stateLabels: { cancelled: 'Cancelled' },
        subStates: [],
        taskCount: tasks.filter((t) => t.status === 'cancelled').length,
        isTerminal: true,
        kind: 'terminal',
        dotColor: 'bg-zinc-400',
        accentColor: 'border-l-zinc-400',
      })
      return stripped
    }
    return visible
  }, [workflowQuery.data, tasks, filterIncludeCancelled])
  const allTasks = useMemo(
    () => tasksQuery.data?.pages.flatMap((p) => p.items) ?? [],
    [tasksQuery.data],
  )
  const filteredTasks = useMemo(
    () =>
      tasks.filter((task) =>
        matchesFilters(task, {
          priorityMin: filterPriorityMin,
          priorityMax: filterPriorityMax,
          types: [],
          blockedOnly: filterBlockedOnly,
        }),
      ),
    [filterBlockedOnly, filterPriorityMax, filterPriorityMin, tasks],
  )
  const grouped = useMemo(
    () => groupByColumns(filteredTasks, boardColumns),
    [filteredTasks, boardColumns],
  )
  const hasActiveFilters =
    filterAgentIds.length > 0 ||
    filterPriorityMin !== undefined ||
    filterPriorityMax !== undefined ||
    filterBlockedOnly ||
    filterIncludeCancelled ||
    filterIncludeArchived

  const activeFilterCount = [
    filterAgentIds.length > 0,
    filterPriorityMin !== undefined || filterPriorityMax !== undefined,
    filterBlockedOnly,
    filterIncludeCancelled,
    filterIncludeArchived,
  ].filter(Boolean).length

  useEffect(() => {
    setFilters({
      agentIds: search.agentIds ? search.agentIds.split(',').filter(Boolean) : [],
      priorityMax: typeof search.priorityMax === 'number' ? search.priorityMax : undefined,
      priorityMin: typeof search.priorityMin === 'number' ? search.priorityMin : undefined,
      q: search.q ?? '',
      blockedOnly: search.blockedOnly === true,
      includeCancelled: search.includeCancelled === true,
      includeArchived: search.includeArchived === true,
    })
  }, [
    setFilters,
    search.agentIds,
    search.blockedOnly,
    search.priorityMax,
    search.priorityMin,
    search.q,
    search.includeCancelled,
    search.includeArchived,
  ])

  useEffect(() => {
    if (lastSyncedAllTasksRef.current === allTasks) return
    lastSyncedAllTasksRef.current = allTasks

    setTasks((current) => {
      if (pendingReorderIds.size === 0) return allTasks

      const serverTasksById = new Map(allTasks.map((task) => [task.id, task]))
      const serverOrderedTasks = allTasks.filter((task) => !pendingReorderIds.has(task.id))
      const usedTaskIds = new Set<string>()
      const mergedTasks: Task[] = []
      let serverIndex = 0

      for (const localTask of current) {
        if (pendingReorderIds.has(localTask.id)) {
          const serverTask = serverTasksById.get(localTask.id)
          if (!serverTask) continue
          mergedTasks.push({ ...serverTask, board_position: localTask.board_position })
          usedTaskIds.add(localTask.id)
          continue
        }

        const nextServerTask = serverOrderedTasks[serverIndex]
        if (nextServerTask) {
          mergedTasks.push(nextServerTask)
          usedTaskIds.add(nextServerTask.id)
          serverIndex += 1
        }
      }

      for (; serverIndex < serverOrderedTasks.length; serverIndex += 1) {
        const serverTask = serverOrderedTasks[serverIndex]
        if (!usedTaskIds.has(serverTask.id)) mergedTasks.push(serverTask)
      }

      return mergedTasks
    })
  }, [allTasks, pendingReorderIds])

  useEffect(() => {
    setSelectedTaskId(search.task)
  }, [search.task])

  useEffect(() => {
    if (!contextMenu) return
    const close = () => setContextMenu(undefined)
    document.addEventListener('click', close)
    window.addEventListener('scroll', close, true)
    return () => {
      document.removeEventListener('click', close)
      window.removeEventListener('scroll', close, true)
    }
  }, [contextMenu])

  useEffect(() => {
    const openCreateDialog = () => setCreateDialogOpen(true)
    const focusSearch = () => {
      searchInputRef.current?.focus()
      searchInputRef.current?.select()
    }
    window.addEventListener('forge:create-task', openCreateDialog)
    window.addEventListener('forge:focus-board-search', focusSearch)
    return () => {
      window.removeEventListener('forge:create-task', openCreateDialog)
      window.removeEventListener('forge:focus-board-search', focusSearch)
    }
  }, [])

  const setUrlFilters = (patch: BoardFilterPatch) => {
    const nextAgentIds = 'agentIds' in patch ? (patch.agentIds ?? []) : filterAgentIds
    const next = {
      priorityMax: filterPriorityMax,
      priorityMin: filterPriorityMin,
      q: filterQ,
      task: search.task,
      blockedOnly: filterBlockedOnly,
      includeCancelled: filterIncludeCancelled,
      includeArchived: filterIncludeArchived,
      ...patch,
      agentIds: nextAgentIds,
    }
    setFilters({
      agentIds: next.agentIds,
      priorityMax: next.priorityMax,
      priorityMin: next.priorityMin,
      q: next.q ?? '',
      blockedOnly: Boolean(next.blockedOnly),
      includeCancelled: Boolean(next.includeCancelled),
      includeArchived: Boolean(next.includeArchived),
    })
    void navigate({
      search: () => ({
        agentIds: next.agentIds.length > 0 ? next.agentIds.join(',') : undefined,
        priorityMax: next.priorityMax,
        priorityMin: next.priorityMin,
        q: next.q || undefined,
        task: next.task || undefined,
        blockedOnly: next.blockedOnly || undefined,
        includeCancelled: next.includeCancelled || undefined,
        includeArchived: next.includeArchived || undefined,
      }),
    })
  }

  const handleAgentClick = (agentId: string) => {
    const next = filterAgentIds.includes(agentId)
      ? filterAgentIds.filter((id) => id !== agentId)
      : [...filterAgentIds, agentId]
    setUrlFilters({ agentIds: next })
  }

  const openTaskDetail = (taskId: string) => {
    setSelectedTaskId(taskId)
    setUrlFilters({ task: taskId })
  }

  const closeTaskDetail = () => {
    setSelectedTaskId(undefined)
    setUrlFilters({ task: undefined })
  }

  const removePendingReorder = (taskId: string) => {
    setPendingReorderIds((current) => {
      const next = new Set(current)
      next.delete(taskId)
      return next
    })
  }

  const reorderWithOptimism = (taskId: string, body: PositionRequest, previousTasks: Task[]) => {
    if (body.before_id === null && body.after_id === null) return
    setPendingReorderIds((current) => new Set(current).add(taskId))
    reorderTask.mutate(
      { taskId, body },
      {
        onError: (error) => {
          setTasks(previousTasks)
          toastApiError(error, 'Reorder failed')
        },
        onSuccess: (result) => {
          setTasks((current) =>
            current.map((candidate) => (candidate.id === result.task.id ? result.task : candidate)),
          )
        },
        onSettled: () => removePendingReorder(taskId),
      },
    )
  }

  const transitionWithOptimism = (
    task: Task,
    toStatus: string,
    beforeTaskId?: string,
    options?: {
      onError?: () => void
      onSuccess?: (task: Task) => void
      previousTasks?: Task[]
      source?: 'board_drag'
    },
  ) => {
    const previousTasks = options?.previousTasks ?? tasks
    setTasks((current) => patchTaskIntoStatus(current, task, toStatus, beforeTaskId))
    transitionTask.mutate(
      {
        taskId: task.id,
        body: { status: toStatus, version: task.version, source: options?.source },
      },
      {
        onError: (error) => {
          setTasks(previousTasks)
          options?.onError?.()
          if (error instanceof ApiError && error.status === 409) {
            toast.error('Version conflict — task was updated')
            return
          }
          if (error instanceof ApiError && error.status === 412) {
            let msg = 'Transition blocked by guard condition'
            try {
              const body = JSON.parse(error.message) as { message?: string }
              if (body.message) msg = body.message
            } catch { /* raw string */ }
            toast.error(msg)
            return
          }
          toastApiError(error, 'Transition failed')
        },
        onSuccess: (result) => {
          const updatedTask = result.task
          setTasks((current) =>
            current.map((candidate) => (candidate.id === updatedTask.id ? updatedTask : candidate)),
          )
          options?.onSuccess?.(updatedTask)
        },
      },
    )
  }

  const getValidTargetPrimaryStates = (
    taskStatus: string,
    workflow: WorkflowDefinition | undefined,
  ): string[] => {
    if (workflow) return getValidDropColumns(workflow, taskStatus)
    return (taskStatusTransitions[taskStatus] ?? []).filter((s) =>
      boardColumns.some((c) => c.primaryState === s),
    )
  }

  const onDragStart = (start: DragStart) => {
    const task = tasks.find((candidate) => candidate.id === start.draggableId)
    setDraggingTaskId(start.draggableId)
    setValidDropStatuses(task ? getValidTargetPrimaryStates(task.status, workflowQuery.data) : [])
  }

  const onDragUpdate = (update: DragUpdate) => {
    setActiveDropStatus(update.destination?.droppableId ?? undefined)
  }

  const onDragEnd = (result: DropResult) => {
    setDraggingTaskId(undefined)
    setValidDropStatuses([])
    setActiveDropStatus(undefined)
    if (!result.destination) return
    const fromColumnId = result.source.droppableId
    const toPrimaryState = result.destination.droppableId
    const sourceCol = boardColumns.find((c) => c.primaryState === fromColumnId)
    if (!sourceCol) return
    const task = (grouped[fromColumnId] ?? [])[result.source.index]
    if (!task) return
    const destinationTasks = (grouped[toPrimaryState] ?? []).filter(
      (candidate) => candidate.id !== task.id,
    )
    const afterTaskId = destinationTasks[result.destination.index]?.id
    const destinationTasksAfterDrop = [
      ...destinationTasks.slice(0, result.destination.index),
      { ...task, status: toPrimaryState },
      ...destinationTasks.slice(result.destination.index),
    ]
    const positionBody: PositionRequest = {
      before_id: destinationTasksAfterDrop[result.destination.index - 1]?.id ?? null,
      after_id: destinationTasksAfterDrop[result.destination.index + 1]?.id ?? null,
    }
    const previousTasks = tasks

    if (fromColumnId === toPrimaryState) {
      setTasks((current) => patchTaskIntoStatus(current, task, task.status, afterTaskId))
      reorderWithOptimism(task.id, positionBody, previousTasks)
      return
    }

    if (positionBody.before_id !== null || positionBody.after_id !== null) {
      setPendingReorderIds((current) => new Set(current).add(task.id))
    }
    transitionWithOptimism(task, toPrimaryState, afterTaskId, {
      previousTasks,
      source: 'board_drag',
      onError: () => removePendingReorder(task.id),
      onSuccess: () => reorderWithOptimism(task.id, positionBody, previousTasks),
    })
  }

  const renderTaskMenuItems: TaskCardMenuRenderer = (task) => (
    <>
      <DropdownMenuItem onClick={() => transitionWithOptimism(task, 'cancelled')}>
        Cancel
      </DropdownMenuItem>
      <DropdownMenuItem
        disabled={task.status === 'done' || task.status === 'cancelled'}
        onClick={() => setAgentPickerTaskId(task.id)}
      >
        Assign Agent
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => openTaskDetail(task.id)}>View Detail</DropdownMenuItem>
    </>
  )

  const assignAgent = (task: Task, agentId: string) => {
    if (!agentId) return
    assignRole.mutate(
      {
        taskId: task.id,
        roleName: 'coder',
        body: { assignee_type: 'agent', assignee_id: agentId },
      },
      {
        onError: (error) => toastApiError(error, 'Agent assignment failed'),
        onSuccess: () => {
          setAgentPickerTaskId(undefined)
        },
      },
    )
  }

  const submitQuickCreate = () => {
    const title = quickCreateTitle.trim()
    const description = quickCreateDescription.trim()
    if (!title || !description || createTask.isPending) return
    createTask.mutate(
      { title, description, task_type: 'task', priority: 0 },
      {
        onError: (error) => {
          toastApiError(error, 'Task creation failed')
        },
        onSuccess: () => {
          setQuickCreateTitle('')
          setQuickCreateDescription('')
          setQuickCreateOpen(false)
        },
      },
    )
  }

  const cancelQuickCreate = () => {
    setQuickCreateTitle('')
    setQuickCreateDescription('')
    setQuickCreateOpen(false)
  }

  const onTaskContextMenu = (event: MouseEvent<HTMLElement>, task: Task) => {
    event.preventDefault()
    setContextMenu({ task, x: event.clientX, y: event.clientY })
  }

  return (
    <div className="flex h-full flex-col gap-4">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <div className="relative">
            <MagnifyingGlass
              size={15}
              className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
            />
            <input
              ref={searchInputRef}
              className="h-8 w-52 rounded-lg border border-input bg-background pl-8 pr-3 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-1 focus:ring-offset-background"
              placeholder="Search tasks..."
              value={filterQ}
              onChange={(e) => setUrlFilters({ q: e.target.value })}
            />
          </div>

          <button
            type="button"
            className={cn(
              'flex md:hidden h-8 cursor-pointer items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors hover:bg-accent',
              hasActiveFilters || showMobileFilters
                ? 'border-foreground/20 bg-foreground/5 text-foreground'
                : 'text-muted-foreground',
            )}
            onClick={() => setShowMobileFilters((v) => !v)}
          >
            <Funnel size={14} />
            Filters
            {activeFilterCount > 0 && (
              <span className="flex h-4 w-4 items-center justify-center rounded-full bg-foreground text-micro text-background">
                {activeFilterCount}
              </span>
            )}
          </button>

          <div className="hidden md:block h-4 w-px bg-border" />

          <div
            className={cn(
              'flex flex-wrap items-center gap-2',
              showMobileFilters ? 'w-full md:w-auto' : 'hidden md:flex',
            )}
          >
            {(agentsQuery.data?.items ?? []).length > 0 && (
              <>
                <AgentFilterGroup
                  agents={agentsQuery.data?.items ?? []}
                  selectedAgentIds={filterAgentIds}
                  onSelect={(agentIds) => setUrlFilters({ agentIds })}
                />
                <div className="h-4 w-px bg-border" />
              </>
            )}

            {[
              { key: 'blockedOnly' as const, label: 'Blocked', active: filterBlockedOnly },
              { key: 'includeCancelled' as const, label: 'Cancelled', active: filterIncludeCancelled },
              { key: 'includeArchived' as const, label: 'Archived', active: filterIncludeArchived },
            ].map(({ key, label, active }) => (
              <button
                key={key}
                type="button"
                className={cn(
                  'flex h-7 cursor-pointer items-center rounded-full px-2.5 text-xs font-medium transition-colors',
                  active
                    ? 'bg-foreground/10 text-foreground ring-1 ring-inset ring-foreground/20'
                    : 'text-muted-foreground hover:bg-accent hover:text-foreground',
                )}
                onClick={() => setUrlFilters({ [key]: !active })}
              >
                {label}
              </button>
            ))}

            <div className="h-4 w-px bg-border" />

            <div className="flex items-center gap-1.5">
              <span className="text-xs font-medium text-muted-foreground">Priority</span>
              <input
                className="h-7 w-16 rounded-lg border bg-background px-2 text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                min={0}
                placeholder="Min"
                type="number"
                value={filterPriorityMin ?? ''}
                onChange={(e) =>
                  setUrlFilters({
                    priorityMin: e.target.value === '' ? undefined : Number(e.target.value),
                  })
                }
              />
              <span className="text-xs text-muted-foreground">–</span>
              <input
                className="h-7 w-16 rounded-lg border bg-background px-2 text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                min={0}
                placeholder="Max"
                type="number"
                value={filterPriorityMax ?? ''}
                onChange={(e) =>
                  setUrlFilters({
                    priorityMax: e.target.value === '' ? undefined : Number(e.target.value),
                  })
                }
              />
            </div>

            {(filterPriorityMin !== undefined || filterPriorityMax !== undefined) && (
              <span className="flex items-center gap-1 rounded-full border border-border bg-muted/50 py-1 pl-2.5 pr-1.5 text-xs text-foreground">
                Priority: {filterPriorityMin ?? '0'}–{filterPriorityMax ?? '∞'}
                <button
                  type="button"
                  className="cursor-pointer text-muted-foreground transition-colors hover:text-foreground"
                  onClick={() => setUrlFilters({ priorityMin: undefined, priorityMax: undefined })}
                >
                  <X size={10} weight="bold" />
                </button>
              </span>
            )}

            {hasActiveFilters && (
              <button
                type="button"
                className="cursor-pointer text-xs text-muted-foreground transition-colors hover:text-foreground"
                onClick={() =>
                  setUrlFilters({
                    agentIds: [],
                    priorityMin: undefined,
                    priorityMax: undefined,
                    blockedOnly: false,
                    includeCancelled: false,
                    includeArchived: false,
                    q: '',
                  })
                }
              >
                Clear all
              </button>
            )}
          </div>
        </div>

        <Button
          size="sm"
          className="h-8 gap-1.5 rounded-lg text-xs"
          onClick={() => setCreateDialogOpen(true)}
        >
          <Plus size={14} weight="bold" />
          New Task
        </Button>
      </div>

      {tasksQuery.isError ? (
        <ErrorBanner
          error={tasksQuery.error}
          fallback="Tasks failed to load"
          onRetry={() => void tasksQuery.refetch()}
        />
      ) : null}

      {!tasksQuery.isLoading && !tasksQuery.isError && allTasks.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <div className="rounded-xl border border-dashed p-12 text-center">
            <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-muted">
              <Kanban size={22} className="text-muted-foreground" />
            </div>
            <p className="text-sm font-semibold">No tasks yet</p>
            <p className="mt-1.5 text-xs text-muted-foreground">
              Create your first task to get started
            </p>
            <Button className="mt-5 rounded-lg" size="sm" onClick={() => setCreateDialogOpen(true)}>
              <Plus size={13} weight="bold" className="mr-1.5" />
              Create task
            </Button>
          </div>
        </div>
      ) : null}

      <DragDropContext onDragEnd={onDragEnd} onDragStart={onDragStart} onDragUpdate={onDragUpdate}>
        <div className="flex min-h-0 flex-1 min-w-[980px] gap-2.5 overflow-x-auto pb-2">
          {boardColumns.map((column) => (
            <KanbanColumn
              key={column.primaryState}
              column={column}
              tasks={grouped[column.primaryState] ?? []}
              dragDisabled={false}
              validDropStatuses={draggingTaskId ? validDropStatuses : []}
              activeDropStatus={activeDropStatus}
              quickCreateOpen={quickCreateOpen}
              quickCreateTitle={quickCreateTitle}
              quickCreateDescription={quickCreateDescription}
              quickCreateDescriptionRef={quickCreateDescriptionRef}
              createPending={createTask.isPending}
              agentPickerTaskId={agentPickerTaskId}
              agents={agentsQuery.data?.items ?? []}
              agentNamesById={agentNamesById}
              claimPending={assignRole.isPending}
              renderTaskMenuItems={renderTaskMenuItems}
              onToggleQuickCreate={() => setQuickCreateOpen((open) => !open)}
              onQuickCreateTitleChange={setQuickCreateTitle}
              onQuickCreateDescriptionChange={setQuickCreateDescription}
              onSubmitQuickCreate={submitQuickCreate}
              onCancelQuickCreate={cancelQuickCreate}
              onAssignAgent={assignAgent}
              onAgentClick={handleAgentClick}
              onTaskClick={(task) => openTaskDetail(task.id)}
              onTaskContextMenu={onTaskContextMenu}
              hasMore={tasksQuery.hasNextPage}
              onLoadMore={handleLoadMore}
            />
          ))}
        </div>
      </DragDropContext>

      {contextMenu && (
        <div
          className="fixed z-50 min-w-[10rem] overflow-hidden rounded-lg border bg-popover p-1 text-popover-foreground shadow-float"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            className="relative flex w-full cursor-pointer select-none items-center rounded-md px-2.5 py-1.5 text-sm outline-none transition-colors hover:bg-accent hover:text-accent-foreground"
            type="button"
            onClick={() => {
              transitionWithOptimism(contextMenu.task, 'cancelled')
              setContextMenu(undefined)
            }}
          >
            Cancel
          </button>
          <button
            className="relative flex w-full cursor-pointer select-none items-center rounded-md px-2.5 py-1.5 text-sm outline-none transition-colors hover:bg-accent hover:text-accent-foreground"
            type="button"
            onClick={() => {
              setAgentPickerTaskId(contextMenu.task.id)
              setContextMenu(undefined)
            }}
          >
            Assign Agent
          </button>
          <button
            className="relative flex w-full cursor-pointer select-none items-center rounded-md px-2.5 py-1.5 text-sm outline-none transition-colors hover:bg-accent hover:text-accent-foreground"
            type="button"
            onClick={() => {
              openTaskDetail(contextMenu.task.id)
              setContextMenu(undefined)
            }}
          >
            View Detail
          </button>
        </div>
      )}

      <TaskCreateDialog
        open={createDialogOpen}
        projectId={projectId}
        onCreated={(task) => {
          openTaskDetail(task.id)
        }}
        onOpenChange={setCreateDialogOpen}
      />

      {selectedTaskId ? (
        <TaskDetailModal
          taskId={selectedTaskId}
          open={Boolean(selectedTaskId)}
          onClose={closeTaskDetail}
        />
      ) : null}
    </div>
  )
}
