import { ArrowDown, ArrowUp, Plus } from '@phosphor-icons/react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { toast } from 'sonner'
import { apiFetch, reorderSubtasks } from '@/api/client'
import { useCreateTask, useExecutionsQuery } from '@/api/hooks'
import { qk } from '@/api/query-keys'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { getApiErrorMessage } from '@/lib/api-error'
import type { PaginatedResponse, Task } from '@/types/generated'

const allProjectTasksKey = 'all-for-subtasks'

type ProjectTasksForSubtasksOptions = {
  includeCancelled?: boolean
}

export function useProjectTasksForSubtasks(
  projectId: string,
  enabled = true,
  options: ProjectTasksForSubtasksOptions = {},
) {
  const includeCancelled = options.includeCancelled ?? false
  return useQuery({
    queryKey: [...qk.projectTasks(projectId), allProjectTasksKey, { includeCancelled }] as const,
    queryFn: () => fetchAllProjectTasks(projectId, includeCancelled),
    enabled: enabled && Boolean(projectId),
  })
}

export function getRootSubtasks(tasks: Task[], taskId: string): Task[] {
  return tasks
    .filter((candidate) => candidate.parent_task_id === taskId)
    .sort((a, b) => {
      const orderA = a.subtask_order ?? Number.MAX_SAFE_INTEGER
      const orderB = b.subtask_order ?? Number.MAX_SAFE_INTEGER
      if (orderA !== orderB) return orderA - orderB
      return a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id)
    })
}

export function hasIncompleteSubtasks(subtasks: Task[]): boolean {
  return subtasks.length > 0 && subtasks.some((subtask) => subtask.status !== 'done')
}

async function fetchAllProjectTasks(
  projectId: string,
  includeCancelled: boolean,
): Promise<Task[]> {
  const tasks: Task[] = []
  let cursor: string | undefined

  do {
    const response = await apiFetch<PaginatedResponse<Task>>(`/projects/${projectId}/tasks`, {
      search: { cursor, include_cancelled: includeCancelled },
    })
    tasks.push(...response.items)
    cursor = response.next_cursor ?? undefined
  } while (cursor)

  return tasks
}

export function TaskSubtasksPanel({ task }: { task: Task }) {
  const queryClient = useQueryClient()
  const tasksQuery = useProjectTasksForSubtasks(task.project_id)
  const executionsQuery = useExecutionsQuery(task.id)
  const createTask = useCreateTask(task.project_id)
  const subtasks = getRootSubtasks(tasksQuery.data ?? [], task.id)
  const hasWorkspace = (executionsQuery.data?.items ?? []).some(
    (execution) => execution.workspace_id,
  )
  const reorderDisabledReason = hasWorkspace
    ? 'Task already has a workspace.'
    : subtasks.some((subtask) => subtask.status !== 'todo')
      ? 'Subtask sequence has started.'
      : undefined
  const reorderMutation = useMutation({
    mutationFn: (orderedIds: string[]) => reorderSubtasks(task.id, { ordered_ids: orderedIds }),
    onSuccess: (updatedTask) => {
      void queryClient.invalidateQueries({ queryKey: qk.task(updatedTask.id) })
      void queryClient.invalidateQueries({ queryKey: qk.projectTasks(updatedTask.project_id) })
      void tasksQuery.refetch()
    },
    onError: (error) => toast.error(getApiErrorMessage(error, 'Subtask reorder failed')),
  })

  const [showCreate, setShowCreate] = useState(false)
  const [newTitle, setNewTitle] = useState('')

  if (task.parent_task_id != null) return null

  const moveSubtask = (index: number, direction: -1 | 1) => {
    const targetIndex = index + direction
    if (targetIndex < 0 || targetIndex >= subtasks.length || reorderDisabledReason) return
    const next = [...subtasks]
    const [moved] = next.splice(index, 1)
    next.splice(targetIndex, 0, moved)
    reorderMutation.mutate(next.map((subtask) => subtask.id))
  }

  const submitCreate = () => {
    if (!newTitle.trim()) return
    createTask.mutate(
      {
        title: newTitle.trim(),
        parent_task_id: task.id,
      },
      {
        onSuccess: () => {
          setNewTitle('')
          setShowCreate(false)
          void tasksQuery.refetch()
        },
        onError: (error) => toast.error(getApiErrorMessage(error, 'Failed to create subtask')),
      },
    )
  }

  return (
    <div className="mt-4 border-t pt-4">
      <div className="mb-2 flex items-center justify-between">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
          Subtasks
        </p>
        <Button
          size="icon"
          variant="ghost"
          className="h-6 w-6"
          onClick={() => setShowCreate((v) => !v)}
        >
          <Plus size={14} />
        </Button>
      </div>

      {showCreate ? (
        <div className="mb-3 space-y-2 rounded-md border p-3">
          <Input
            autoFocus
            placeholder="Subtask title"
            value={newTitle}
            className="h-8"
            onChange={(e) => setNewTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submitCreate()
              if (e.key === 'Escape') setShowCreate(false)
            }}
          />
          <div className="flex gap-2">
            <Button size="sm" disabled={!newTitle.trim() || createTask.isPending} onClick={submitCreate}>
              Add
            </Button>
            <Button size="sm" variant="outline" onClick={() => setShowCreate(false)}>
              Cancel
            </Button>
          </div>
        </div>
      ) : null}

      {tasksQuery.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
        </div>
      ) : subtasks.length === 0 ? (
        <p className="text-sm text-muted-foreground">No subtasks</p>
      ) : (
        <div className="space-y-2">
          {subtasks.map((subtask, index) => {
            const canMoveUp = index > 0 && !reorderDisabledReason
            const canMoveDown = index < subtasks.length - 1 && !reorderDisabledReason
            return (
              <div key={subtask.id} className="rounded-md border bg-background p-2">
                <div className="flex items-start gap-2">
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{subtask.title}</p>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      {subtask.status} - order {subtask.subtask_order ?? index + 1}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <span title={!canMoveUp ? reorderDisabledReason : undefined}>
                      <Button
                        aria-label="Move subtask up"
                        className="h-7 w-7"
                        disabled={!canMoveUp || reorderMutation.isPending}
                        size="icon"
                        variant="ghost"
                        onClick={() => moveSubtask(index, -1)}
                      >
                        <ArrowUp size={14} />
                      </Button>
                    </span>
                    <span title={!canMoveDown ? reorderDisabledReason : undefined}>
                      <Button
                        aria-label="Move subtask down"
                        className="h-7 w-7"
                        disabled={!canMoveDown || reorderMutation.isPending}
                        size="icon"
                        variant="ghost"
                        onClick={() => moveSubtask(index, 1)}
                      >
                        <ArrowDown size={14} />
                      </Button>
                    </span>
                  </div>
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
