import { Badge } from '@/components/ui/badge'
import type { Task } from '@/types/generated'

export function TaskRepositoryPanel({ task }: { task: Task }) {
  const repositories = [...(task.repositories ?? [])].sort(
    (left, right) => left.position - right.position,
  )

  return (
    <section
      aria-labelledby="task-repositories-title"
      className="min-w-0 space-y-3 rounded-lg border p-4 lg:col-span-2"
    >
      <div>
        <h2 id="task-repositories-title" className="font-semibold">
          Task repositories
        </h2>
        <p className="mt-1 text-xs text-muted-foreground">
          Repository roles and available integration or pull request status for this Task.
        </p>
      </div>
      {repositories.length ? (
        <ul className="space-y-2">
          {repositories.map((repository) => {
            const writable = repository.role === 'primary' || repository.role === 'affected'

            return (
              <li
                key={repository.repo_id}
                className="flex min-w-0 flex-col gap-2 rounded-md border bg-muted/10 p-3 sm:flex-row sm:items-start sm:justify-between"
              >
                <div className="min-w-0">
                  <p className="break-all text-sm font-medium">{repository.repo_id}</p>
                </div>
                <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs">
                  <Badge variant="outline" className="capitalize">
                    {repository.role}
                  </Badge>
                  {!writable ? <span className="text-muted-foreground">Read only</span> : null}
                  {repository.merge ? (
                    <span>
                      Integration: {repository.merge.state.replaceAll('_', ' ')}
                    </span>
                  ) : null}
                  {repository.pull_request ? (
                    repository.pull_request.pr_url ? (
                      <a
                        href={repository.pull_request.pr_url}
                        target="_blank"
                        rel="noreferrer noopener"
                        className="font-medium text-primary underline-offset-4 hover:underline"
                      >
                        Pull request · {repository.pull_request.pr_state.replaceAll('_', ' ')} ·{' '}
                        {repository.pull_request.merge_status.replaceAll('_', ' ')}
                      </a>
                    ) : (
                      <span>
                        Pull request: {repository.pull_request.pr_state.replaceAll('_', ' ')} ·{' '}
                        {repository.pull_request.merge_status.replaceAll('_', ' ')}
                      </span>
                    )
                  ) : null}
                  {writable && !repository.merge && !repository.pull_request ? (
                    <span className="text-muted-foreground">
                      No integration or pull request status recorded.
                    </span>
                  ) : null}
                </div>
              </li>
            )
          })}
        </ul>
      ) : (
        <p className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
          No repositories are associated with this Task.
        </p>
      )}
    </section>
  )
}
