import { useEffect, useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { toast } from 'sonner'
import { useCreateTask, useTasksQuery, useTransitionTaskLifecycle } from '@/api/hooks'
import { ErrorBanner } from '@/components/error-banner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import type { Task, TaskLifecycleState } from '@/types/generated'

const lanes: TaskLifecycleState[] = [
  'backlog', 'ready', 'active', 'blocked', 'ready_to_merge', 'merging', 'done', 'cancelled',
]

const nextStates: Partial<Record<TaskLifecycleState, TaskLifecycleState[]>> = {
  backlog: ['ready', 'blocked', 'cancelled'],
  ready: ['active', 'blocked', 'cancelled'],
  active: ['ready', 'blocked', 'cancelled'],
  blocked: ['ready', 'active', 'cancelled'],
}

export function BoardPage({ projectId }: { projectId: string }) {
  const navigate = useNavigate()
  const [queryText, setQueryText] = useState('')
  const [includeCancelled, setIncludeCancelled] = useState(false)
  const [createOpen, setCreateOpen] = useState(false)
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const tasksQuery = useTasksQuery(projectId, {
    q: queryText.trim() || undefined,
    lifecycle_state: lanes.join(','),
    sort_by: 'board_position',
    sort_order: 'asc',
    limit: 200,
  })
  const createTask = useCreateTask(projectId)
  const transition = useTransitionTaskLifecycle()
  const allTasks = tasksQuery.data?.pages.flatMap((page) => page.items) ?? []
  const tasks = includeCancelled ? allTasks : allTasks.filter((task) => task.lifecycle.state !== 'cancelled')

  useEffect(() => {
    const open = () => setCreateOpen(true)
    const focus = () => document.getElementById('board-search')?.focus()
    window.addEventListener('forge:create-task', open)
    window.addEventListener('forge:focus-board-search', focus)
    return () => {
      window.removeEventListener('forge:create-task', open)
      window.removeEventListener('forge:focus-board-search', focus)
    }
  }, [])

  async function create() {
    const nextTitle = title.trim()
    if (!nextTitle) return
    try {
      const task = await createTask.mutateAsync({
        title: nextTitle,
        description: description.trim() || undefined,
        task_type: 'implementation',
        priority: 0,
      })
      setTitle('')
      setDescription('')
      setCreateOpen(false)
      void navigate({ to: '/tasks/$taskId', params: { taskId: task.id } })
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Task creation failed'))
    }
  }

  async function move(task: Task, toState: TaskLifecycleState) {
    try {
      await transition.mutateAsync({
        taskId: task.id,
        toState,
        expectedLifecycleVersion: task.lifecycle.version,
        idempotencyKey: crypto.randomUUID(),
      })
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Task lifecycle transition failed'))
    }
  }

  return (
    <main className="flex h-full min-h-0 flex-col gap-4 p-4 sm:p-6" data-board-page>
      <header className="flex flex-wrap items-end gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="text-2xl font-semibold">Task board</h1>
          <p className="mt-1 text-sm text-muted-foreground">Columns show aggregate TaskLifecycle states.</p>
        </div>
        <Input id="board-search" className="max-w-xs" value={queryText} onChange={(event) => setQueryText(event.target.value)} placeholder="Search tasks" aria-label="Search tasks" />
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={includeCancelled} onChange={(event) => setIncludeCancelled(event.target.checked)} /> Cancelled</label>
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

      {tasksQuery.isError ? <ErrorBanner error={tasksQuery.error} fallback="Task board failed to load" onRetry={() => void tasksQuery.refetch()} /> : null}
      {tasksQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading board…</p> : null}
      <section className="grid min-h-0 flex-1 grid-cols-1 gap-3 overflow-x-auto pb-2 sm:grid-cols-2 xl:grid-cols-4 2xl:grid-cols-8">
        {lanes.filter((state) => includeCancelled || state !== 'cancelled').map((state) => {
          const laneTasks = tasks.filter((task) => task.lifecycle.state === state)
          return (
            <div key={state} className="flex min-h-40 min-w-[220px] flex-col rounded-lg border bg-muted/15">
              <div className="flex items-center justify-between border-b px-3 py-2">
                <h2 className="text-sm font-semibold capitalize">{state.replaceAll('_', ' ')}</h2>
                <span className="rounded-full bg-muted px-2 py-0.5 text-xs">{laneTasks.length}</span>
              </div>
              <div className="flex flex-1 flex-col gap-2 overflow-y-auto p-2">
                {laneTasks.map((task) => (
                  <article key={task.id} className="space-y-3 rounded-md border bg-card p-3 shadow-sm">
                    <button type="button" className="block w-full text-left" onClick={() => void navigate({ to: '/tasks/$taskId', params: { taskId: task.id } })}>
                      <strong className="block text-sm">{task.title}</strong>
                      <span className="mt-1 block text-xs text-muted-foreground">{task.task_type} · priority {task.priority}</span>
                    </button>
                    <div className="flex flex-wrap gap-1">
                      {(nextStates[state] ?? []).map((next) => (
                        <Button key={next} size="sm" variant="outline" className="h-7 px-2 text-xs" disabled={transition.isPending} onClick={() => void move(task, next)}>
                          {next.replaceAll('_', ' ')}
                        </Button>
                      ))}
                    </div>
                  </article>
                ))}
                {laneTasks.length === 0 ? <p className="p-2 text-xs text-muted-foreground">No tasks</p> : null}
              </div>
            </div>
          )
        })}
      </section>
      {tasksQuery.hasNextPage ? <div className="flex justify-center"><Button variant="outline" disabled={tasksQuery.isFetchingNextPage} onClick={() => void tasksQuery.fetchNextPage()}>{tasksQuery.isFetchingNextPage ? 'Loading…' : 'Load more'}</Button></div> : null}
    </main>
  )
}
