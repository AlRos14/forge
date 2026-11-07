import { useState, type FormEvent } from 'react'
import {
  useAgentsInfiniteQuery,
  useCreateTask,
  useDaemonsQuery,
  useMembersQuery,
  useProjectQuery,
  useProjectTaskRoleDefaultsQuery,
} from '@/api/hooks'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { MarkdownEditor } from '@/components/ui/markdown-editor'
import { ActorIdentity } from '@/features/task-roles/ActorIdentity'
import { toastApiError } from '@/lib/api-error'
import { splitTaskContent } from '@/lib/task-content'
import type { Task } from '@/types/generated'

interface TaskCreateDialogProps {
  open: boolean
  projectId: string
  onOpenChange: (open: boolean) => void
  onCreated?: (task: Task) => void
}

export function TaskCreateDialog({
  open,
  projectId,
  onOpenChange,
  onCreated,
}: TaskCreateDialogProps) {
  const [content, setContent] = useState('')
  const createTask = useCreateTask(projectId)
  const projectQuery = useProjectQuery(projectId, open)
  const defaultsQuery = useProjectTaskRoleDefaultsQuery(projectId, open)
  const membersQuery = useMembersQuery(projectId, open)
  const agentsQuery = useAgentsInfiniteQuery(100, open)
  const daemonsQuery = useDaemonsQuery(open)
  const members = membersQuery.data ?? []
  const agents = (agentsQuery.data?.pages.flatMap((page) => page.items) ?? []).filter(
    (agent) => projectQuery.data?.owner_id != null && agent.owner_id === projectQuery.data.owner_id,
  )
  const daemons = daemonsQuery.data?.items ?? []

  const reset = () => {
    setContent('')
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const taskContent = splitTaskContent(content)
    if (!content.trim()) return

    createTask.mutate(
      {
        ...taskContent,
        priority: 0,
      },
      {
        onError: (error) => toastApiError(error, 'Task creation failed'),
        onSuccess: (task) => {
          onCreated?.(task)
          reset()
          onOpenChange(false)
        },
      },
    )
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">
        <form className="space-y-4" onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>Create Task</DialogTitle>
          </DialogHeader>
          <div className="space-y-2">
            <Label>Objective and constraints</Label>
            <MarkdownEditor
              ariaLabel="Task"
              autoFocus
              placeholder={
                'Describe the task… Add a first-level heading like “# Plan FirePOS” if you want to set its title.'
              }
              minHeight="240px"
              value={content}
              onChange={setContent}
            />
            <p className="text-xs text-muted-foreground">
              A first-level “# heading” becomes the title. Without one, the first line becomes the
              title. The remaining text is saved as the Markdown description.
            </p>
          </div>
          <section
            className="space-y-3 rounded-lg border border-border-subtle bg-muted/20 p-4"
            aria-labelledby="task-team-preview-title"
          >
            <div>
              <h3 id="task-team-preview-title" className="text-sm font-semibold">
                Project team for this Task
              </h3>
              <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                New Tasks copy the saved Project roles and members. Later Project changes do not
                rewrite this Task.
              </p>
            </div>
            {defaultsQuery.isLoading ||
            membersQuery.isLoading ||
            agentsQuery.isLoading ||
            daemonsQuery.isLoading ? (
              <p className="text-sm text-muted-foreground">Loading the saved Project team…</p>
            ) : null}
            {defaultsQuery.isError || membersQuery.isError || agentsQuery.isError ? (
              <div
                role="alert"
                className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
              >
                <span>
                  The saved Project team could not be loaded. Task creation remains available.
                </span>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    void Promise.all([
                      defaultsQuery.refetch(),
                      membersQuery.refetch(),
                      agentsQuery.refetch(),
                    ])
                  }
                >
                  Retry
                </Button>
              </div>
            ) : null}
            {defaultsQuery.data ? (
              defaultsQuery.data.roles.length ? (
                <ul className="grid gap-2 sm:grid-cols-2">
                  {defaultsQuery.data.roles.map((role) => (
                    <li key={role.role} className="min-w-0 rounded-md border bg-background p-3">
                      <div className="flex flex-wrap items-center gap-2">
                        <h4 className="min-w-0 truncate text-sm font-medium">
                          {role.configuration.display_label || role.role.replaceAll('_', ' ')}
                        </h4>
                        {role.configuration.capabilities.includes('orchestration') ? (
                          <span className="rounded-full bg-[var(--ember-surface)] px-2 py-0.5 text-[10px] font-medium text-primary">
                            Coordinator
                          </span>
                        ) : null}
                      </div>
                      <div className="mt-2 space-y-2">
                        {role.members.map((actor) => (
                          <ActorIdentity
                            key={`${actor.kind}:${actor.id}`}
                            actor={actor}
                            agents={agents}
                            members={members}
                            daemons={daemons}
                          />
                        ))}
                        {!role.members.length ? (
                          <p className="text-xs text-muted-foreground">
                            No default members for this role yet.
                          </p>
                        ) : null}
                      </div>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
                  This Project has no saved roles yet. They can be added in Project settings.
                </p>
              )
            ) : null}
            {daemonsQuery.isError ? (
              <p className="text-xs text-muted-foreground">
                Host details are unavailable; Agent names and harnesses remain visible.
              </p>
            ) : null}
          </section>
          <DialogFooter>
            <Button
              disabled={createTask.isPending}
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button disabled={createTask.isPending || !content.trim()} type="submit">
              {createTask.isPending ? 'Creating...' : 'Create Task'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
