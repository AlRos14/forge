import { useEffect, useState, type FormEvent } from 'react'
import { Link } from '@tanstack/react-router'
import { apiFetch } from '@/api/client'
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
import type {
  PaginatedResponse,
  Repo,
  Task,
  TaskRepositoryInput,
  TaskRepositoryRole,
} from '@/types/generated'

interface TaskCreateDialogProps {
  open: boolean
  projectId: string
  onOpenChange: (open: boolean) => void
  onCreated?: (task: Task) => void
}

function projectRepositoryRoles(
  repos: Repo[],
  primaryRepoId: string | null | undefined,
): Record<string, TaskRepositoryRole | ''> {
  return repos.reduce<Record<string, TaskRepositoryRole | ''>>((roles, repo) => {
    roles[repo.id] = repo.id === primaryRepoId ? 'primary' : ''
    return roles
  }, {})
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
  const [repos, setRepos] = useState<Repo[]>([])
  const [reposLoading, setReposLoading] = useState(false)
  const [reposLoaded, setReposLoaded] = useState(false)
  const [reposError, setReposError] = useState(false)
  const [repositoryRoles, setRepositoryRoles] = useState<Record<string, TaskRepositoryRole | ''>>(
    {},
  )

  useEffect(() => {
    if (!open) return
    let active = true
    setRepos([])
    setReposLoaded(false)
    setReposError(false)
    setReposLoading(true)
    void apiFetch<PaginatedResponse<Repo>>(`/projects/${projectId}/repos`)
      .then((response) => {
        if (!active) return
        setRepos(response.items)
        setReposLoaded(true)
      })
      .catch(() => {
        if (active) setReposError(true)
      })
      .finally(() => {
        if (active) setReposLoading(false)
      })
    return () => {
      active = false
    }
  }, [open, projectId])

  useEffect(() => {
    if (!open || !reposLoaded || projectQuery.isLoading) return
    setRepositoryRoles(projectRepositoryRoles(repos, projectQuery.data?.primary_repo_id))
  }, [open, projectQuery.data?.primary_repo_id, projectQuery.isLoading, reposLoaded, repos])

  const repositoryInputs: TaskRepositoryInput[] = repos.flatMap((repo) => {
    const role = repositoryRoles[repo.id]
    return role ? [{ repo_id: repo.id, role }] : []
  })
  const hasRepositoryPrimary = repositoryInputs.some((repository) => repository.role === 'primary')
  const repositorySelectionInvalid = repositoryInputs.length > 0 && !hasRepositoryPrimary

  const reset = () => {
    setContent('')
    setRepositoryRoles(projectRepositoryRoles(repos, projectQuery.data?.primary_repo_id))
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const taskContent = splitTaskContent(content)
    if (!content.trim()) return

    createTask.mutate(
      {
        ...taskContent,
        priority: 0,
        ...(reposLoaded && !projectQuery.isLoading ? { repositories: repositoryInputs } : {}),
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
          <section className="space-y-3 rounded-lg border border-border-subtle bg-muted/20 p-4">
            <div>
              <h3 className="text-sm font-semibold">Task repositories</h3>
              <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                Choose one primary repository and mark any additional repositories as affected or
                context. Context repositories are read-only. Leave every repository unselected to
                create a repo-less Task.
              </p>
            </div>
            {reposLoading ? (
              <p className="text-sm text-muted-foreground">Loading Project repositories…</p>
            ) : null}
            {reposError ? (
              <p role="alert" className="text-sm text-destructive">
                Project repositories could not be loaded. This Task will use the Project default.
              </p>
            ) : null}
            {reposLoaded && repos.length ? (
              <div className="space-y-2">
                {repos.map((repo) => (
                  <label key={repo.id} className="grid gap-2 text-sm sm:grid-cols-[1fr_12rem]">
                    <span className="self-center truncate">{repo.name}</span>
                    <select
                      aria-label={`Repository role for ${repo.name}`}
                      className="h-9 rounded-md border bg-background px-2 text-sm"
                      value={repositoryRoles[repo.id] ?? ''}
                      disabled={projectQuery.isLoading}
                      onChange={(event) => {
                        const nextRole = event.target.value as TaskRepositoryRole | ''
                        setRepositoryRoles((current) => {
                          const next = { ...current, [repo.id]: nextRole }
                          if (nextRole === 'primary') {
                            for (const otherRepo of repos) {
                              if (otherRepo.id !== repo.id && next[otherRepo.id] === 'primary') {
                                next[otherRepo.id] = 'affected'
                              }
                            }
                          }
                          return next
                        })
                      }}
                    >
                      <option value="">Not part of this Task</option>
                      <option value="primary">Primary</option>
                      <option value="affected">Affected · writable</option>
                      <option value="context">Context · read only</option>
                    </select>
                  </label>
                ))}
                {repositorySelectionInvalid ? (
                  <p role="alert" className="text-xs text-destructive">
                    Choose a primary repository when adding affected or context repositories.
                  </p>
                ) : null}
              </div>
            ) : null}
            {reposLoaded && repos.length === 0 ? (
              <p className="text-sm text-muted-foreground">No repositories are configured.</p>
            ) : null}
          </section>
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
                <div className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
                  <p>
                    This Project has no saved roles yet. New Tasks copy the Project team, so add
                    roles and templates once in Project settings.
                  </p>
                  <Link
                    to="/projects/$projectId/settings/$tab"
                    params={{ projectId, tab: 'workers' }}
                    className="mt-2 inline-flex items-center text-sm font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    Manage Project team &amp; templates
                  </Link>
                </div>
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
            <Button
              disabled={createTask.isPending || !content.trim() || repositorySelectionInvalid}
              type="submit"
            >
              {createTask.isPending ? 'Creating...' : 'Create Task'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
