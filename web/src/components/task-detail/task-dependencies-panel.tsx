import { GitFork, Plus, X } from '@phosphor-icons/react'
import { useNavigate } from '@tanstack/react-router'
import { useState, useMemo } from 'react'
import { toast } from 'sonner'
import {
  useAddDependency,
  useRemoveDependency,
  useTaskDependenciesQuery,
  useTaskDependentsQuery,
} from '@/api/hooks'
import { TaskStatusBadge } from '@/components/task-controls'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { getApiErrorMessage } from '@/lib/api-error'
import type { Task } from '@/types/generated'
import { useProjectTasksForSubtasks } from './task-subtasks-panel'

interface TaskDependenciesPanelProps {
  task: Task
}

export function TaskDependenciesPanel({ task }: TaskDependenciesPanelProps) {
  const navigate = useNavigate()
  const dependenciesQuery = useTaskDependenciesQuery(task.id)
  const dependentsQuery = useTaskDependentsQuery(task.id)
  const projectTasksQuery = useProjectTasksForSubtasks(task.project_id, true, {
    includeCancelled: true,
  })
  const addDependency = useAddDependency(task.id)
  const removeDependency = useRemoveDependency(task.id)

  const [showPicker, setShowPicker] = useState(false)
  const [search, setSearch] = useState('')

  const allTasks = projectTasksQuery.data ?? []
  const taskById = useMemo(() => new Map(allTasks.map((t) => [t.id, t])), [allTasks])
  const dependencyRows = dependenciesQuery.data ?? []
  const depIds = useMemo(
    () => new Set(dependencyRows.map((d) => d.depends_on_id)),
    [dependencyRows],
  )

  const dependencyTasks = useMemo(
    () =>
      dependencyRows.map((dependency) => ({
        dependency,
        task: taskById.get(dependency.depends_on_id),
      })),
    [dependencyRows, taskById],
  )

  const dependentTaskIds = new Set((dependentsQuery.data ?? []).map((d) => d.task_id))
  const dependentTasks = useMemo(
    () => allTasks.filter((t) => dependentTaskIds.has(t.id)),
    [allTasks, dependentTaskIds],
  )

  const isTerminal = task.status === 'done' || task.status === 'cancelled'

  const candidateTasks = useMemo(() => {
    const q = search.toLowerCase()
    return allTasks.filter(
      (t) =>
        t.id !== task.id &&
        !depIds.has(t.id) &&
        !dependentTaskIds.has(t.id) &&
        t.status !== 'cancelled' &&
        (q === '' || t.title.toLowerCase().includes(q)),
    )
  }, [allTasks, task.id, depIds, dependentTaskIds, search])

  const handleAdd = (dependsOnId: string) => {
    addDependency.mutate(dependsOnId, {
      onSuccess: () => {
        setShowPicker(false)
        setSearch('')
      },
      onError: (error) => {
        toast.error(getApiErrorMessage(error, 'Failed to add dependency'))
      },
    })
  }

  const handleRemove = (dependsOnId: string) => {
    removeDependency.mutate(dependsOnId, {
      onError: (error) => toast.error(getApiErrorMessage(error, 'Failed to remove dependency')),
    })
  }

  const openTaskDetail = (taskId: string) => {
    void navigate({ to: '/tasks/$taskId', params: { taskId } })
  }

  const isLoading = dependenciesQuery.isLoading || projectTasksQuery.isLoading

  return (
    <div className="mt-4 border-t pt-4">
      <div className="mb-2 flex items-center justify-between">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
          Dependencies
        </p>
        {!isTerminal && (
          <Button
            size="icon"
            variant="ghost"
            className="h-6 w-6"
            aria-label="Add dependency"
            onClick={() => {
              setShowPicker((v) => !v)
              setSearch('')
            }}
          >
            <Plus size={14} />
          </Button>
        )}
      </div>

      {showPicker && (
        <div className="mb-3 rounded-md border bg-background p-2">
          <input
            autoFocus
            type="text"
            placeholder="Search tasks…"
            className="mb-1.5 w-full rounded border bg-muted/40 px-2 py-1 text-sm outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-ring"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                setShowPicker(false)
                setSearch('')
              }
            }}
          />
          <div className="max-h-40 overflow-y-auto">
            {projectTasksQuery.isLoading ? (
              <div className="space-y-1 p-1">
                <Skeleton className="h-7 w-full" />
                <Skeleton className="h-7 w-full" />
              </div>
            ) : candidateTasks.length === 0 ? (
              <p className="px-2 py-1.5 text-xs text-muted-foreground">No matching tasks</p>
            ) : (
              candidateTasks.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  className="flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={addDependency.isPending}
                  onClick={() => handleAdd(t.id)}
                >
                  <span className="min-w-0 flex-1 truncate">{t.title}</span>
                  <TaskStatusBadge status={t.status} />
                </button>
              ))
            )}
          </div>
          <Button
            size="sm"
            variant="ghost"
            className="mt-1 h-6 w-full text-xs text-muted-foreground"
            onClick={() => {
              setShowPicker(false)
              setSearch('')
            }}
          >
            Cancel
          </Button>
        </div>
      )}

      {isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-full" />
        </div>
      ) : dependencyTasks.length === 0 ? (
        <p className="text-sm text-muted-foreground">No dependencies</p>
      ) : (
        <div className="space-y-1.5">
          {dependencyTasks.map(({ dependency, task: dep }) => (
            <div
              key={dependency.depends_on_id}
              className="group flex items-center gap-2 rounded-md border bg-background px-2 py-1.5"
            >
              <button
                type="button"
                className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-sm text-left transition-colors hover:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                onClick={() => openTaskDetail(dependency.depends_on_id)}
              >
                <span className="min-w-0 flex-1 truncate text-sm">
                  {dep?.title ?? dependency.depends_on_id}
                </span>
                {dep ? (
                  <TaskStatusBadge status={dep.status} />
                ) : (
                  <span className="shrink-0 rounded border px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
                    Missing
                  </span>
                )}
              </button>
              {!isTerminal && (
                <button
                  type="button"
                  aria-label={`Remove dependency on ${dep?.title ?? dependency.depends_on_id}`}
                  className="ml-0.5 shrink-0 rounded p-0.5 text-muted-foreground transition-colors hover:text-destructive"
                  disabled={removeDependency.isPending}
                  onClick={() => handleRemove(dependency.depends_on_id)}
                >
                  <X size={12} />
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {dependentTasks.length > 0 && (
        <div className="mt-3">
          <div className="mb-1.5 flex items-center gap-1 text-xs text-muted-foreground">
            <GitFork size={11} />
            <span className="uppercase tracking-wide font-medium">Blocking</span>
          </div>
          <div className="space-y-1.5">
            {dependentTasks.map((dep) => (
              <button
                key={dep.id}
                type="button"
                className="flex w-full cursor-pointer items-center gap-2 rounded-md border bg-muted/30 px-2 py-1.5 text-left transition-colors hover:border-border hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                onClick={() => openTaskDetail(dep.id)}
              >
                <span className="min-w-0 flex-1 truncate text-sm text-muted-foreground">
                  {dep.title}
                </span>
                <TaskStatusBadge status={dep.status} />
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
