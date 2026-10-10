import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { toast } from 'sonner'
import { useCreateTask, useTasksQuery } from '@/api/hooks'
import { ErrorBanner } from '@/components/error-banner'
import { getApiErrorMessage } from '@/lib/api-error'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import type { TaskLifecycleState } from '@/types/generated'

export type TaskListSortBy = 'title' | 'lifecycle_state' | 'priority' | 'task_type' | 'updated_at' | 'board_position'
export type TaskListSortOrder = 'asc' | 'desc'

const lifecycleStates: Array<TaskLifecycleState | ''> = [
  '', 'backlog', 'ready', 'active', 'blocked', 'ready_to_merge', 'merging', 'done', 'cancelled',
]

export function TaskListPage({
  projectId,
  sortBy,
  sortOrder,
  lifecycleState,
  onSortChange,
  onFilterChange,
}: {
  projectId: string
  sortBy: TaskListSortBy
  sortOrder: TaskListSortOrder
  lifecycleState?: TaskLifecycleState
  onSortChange: (sortBy: TaskListSortBy, sortOrder: TaskListSortOrder) => void
  onFilterChange: (lifecycleState?: TaskLifecycleState) => void
}) {
  const [queryText, setQueryText] = useState('')
  const [createOpen, setCreateOpen] = useState(false)
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const createTask = useCreateTask(projectId)
  const tasksQuery = useTasksQuery(projectId, {
    q: queryText.trim() || undefined,
    lifecycle_state: lifecycleState,
    sort_by: sortBy,
    sort_order: sortOrder,
    limit: 100,
  })
  const tasks = tasksQuery.data?.pages.flatMap((page) => page.items) ?? []

  async function create() {
    const nextTitle = title.trim()
    if (!nextTitle) return
    try {
      await createTask.mutateAsync({
        title: nextTitle,
        description: description.trim() || undefined,
        task_type: 'implementation',
        priority: 0,
      })
      setTitle('')
      setDescription('')
      setCreateOpen(false)
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Task creation failed'))
    }
  }

  return (
    <main className="mx-auto w-full max-w-6xl space-y-5 p-4 sm:p-6">
      <header className="flex flex-wrap items-end gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="text-2xl font-semibold">Tasks</h1>
          <p className="mt-1 text-sm text-muted-foreground">Aggregate lifecycle, role memberships, and exact work records.</p>
        </div>
        <Button onClick={() => setCreateOpen((open) => !open)}>New task</Button>
      </header>

      {createOpen ? (
        <section className="grid gap-3 rounded-lg border p-4 sm:grid-cols-2">
          <Input autoFocus value={title} onChange={(event) => setTitle(event.target.value)} placeholder="Task title" aria-label="Task title" />
          <Textarea value={description} onChange={(event) => setDescription(event.target.value)} placeholder="Description (optional)" aria-label="Task description" />
          <div className="flex gap-2 sm:col-span-2">
            <Button disabled={!title.trim() || createTask.isPending} onClick={() => void create()}>{createTask.isPending ? 'Creating…' : 'Create task'}</Button>
            <Button variant="outline" onClick={() => setCreateOpen(false)}>Cancel</Button>
          </div>
        </section>
      ) : null}

      <section className="flex flex-wrap gap-3 rounded-lg border p-3">
        <Input className="max-w-xs" value={queryText} onChange={(event) => setQueryText(event.target.value)} placeholder="Search tasks" aria-label="Search tasks" />
        <label className="flex items-center gap-2 text-sm">
          <span>TaskLifecycle</span>
          <select className="h-9 rounded-md border bg-background px-2" value={lifecycleState ?? ''} onChange={(event) => onFilterChange((event.target.value || undefined) as TaskLifecycleState | undefined)}>
            {lifecycleStates.map((state) => <option key={state || 'all'} value={state}>{state ? state.replaceAll('_', ' ') : 'All states'}</option>)}
          </select>
        </label>
        <label className="flex items-center gap-2 text-sm">
          <span>Sort</span>
          <select className="h-9 rounded-md border bg-background px-2" value={`${sortBy}:${sortOrder}`} onChange={(event) => {
            const [key, order] = event.target.value.split(':') as [TaskListSortBy, TaskListSortOrder]
            onSortChange(key, order)
          }}>
            <option value="updated_at:desc">Recently updated</option>
            <option value="title:asc">Title</option>
            <option value="priority:desc">Priority</option>
            <option value="lifecycle_state:asc">TaskLifecycle</option>
            <option value="task_type:asc">Task type</option>
          </select>
        </label>
      </section>

      {tasksQuery.isError ? <ErrorBanner error={tasksQuery.error} fallback="Tasks failed to load" onRetry={() => void tasksQuery.refetch()} /> : null}
      <section className="overflow-hidden rounded-lg border">
        <div className="grid grid-cols-[minmax(0,1fr)_140px_120px_90px] gap-3 border-b bg-muted/40 px-4 py-2 text-xs font-medium text-muted-foreground">
          <span>Task</span><span>TaskLifecycle</span><span>Type</span><span>Priority</span>
        </div>
        {tasksQuery.isLoading ? <p className="p-4 text-sm text-muted-foreground">Loading tasks…</p> : null}
        {tasks.map((task) => (
          <Link
            key={task.id}
            to="/tasks/$taskId"
            params={{ taskId: task.id }}
            className="grid grid-cols-[minmax(0,1fr)_140px_120px_90px] items-center gap-3 border-b px-4 py-3 text-sm last:border-b-0 hover:bg-muted/30"
          >
            <span className="min-w-0"><strong className="block truncate">{task.title}</strong><span className="font-mono text-[11px] text-muted-foreground">{task.id}</span></span>
            <span className="capitalize">{task.lifecycle.state.replaceAll('_', ' ')}</span>
            <span>{task.task_type}</span>
            <span>{task.priority}</span>
          </Link>
        ))}
        {!tasksQuery.isLoading && tasks.length === 0 ? <p className="p-6 text-center text-sm text-muted-foreground">No tasks match these filters.</p> : null}
        {tasksQuery.hasNextPage ? <div className="flex justify-center border-t p-3"><Button variant="outline" disabled={tasksQuery.isFetchingNextPage} onClick={() => void tasksQuery.fetchNextPage()}>{tasksQuery.isFetchingNextPage ? 'Loading…' : 'Load more'}</Button></div> : null}
      </section>
    </main>
  )
}
