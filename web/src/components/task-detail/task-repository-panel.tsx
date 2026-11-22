import { Badge } from '@/components/ui/badge'
import type { Task } from '@/types/generated'

export function TaskRepositoryPanel({ task }: { task: Task }) {
  const repositories = [...(task.repositories ?? [])].sort(
    (left, right) => left.position - right.position,
  )
  const mergeBatches = task.repository_merge_batches ?? []

  return (
    <section
      aria-labelledby="task-repositories-title"
      className="min-w-0 space-y-3 rounded-lg border p-4 lg:col-span-2"
    >
      <div>
        <h2 id="task-repositories-title" className="font-semibold">
          Task repositories · revision {task.repository_set_revision ?? 0}
        </h2>
        <p className="mt-1 text-xs text-muted-foreground">
          Repository roles, per repository change state, integration progress and pull request
          status.
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
                  <Badge variant={repository.change_state === 'pending' ? 'secondary' : 'outline'}>
                    {repository.change_state.replaceAll('_', ' ')}
                  </Badge>
                  {!writable ? <span className="text-muted-foreground">Read only</span> : null}
                  {repository.integration ? (
                    <span>
                      Integration: {repository.integration.outcome.replaceAll('_', ' ')}
                      {repository.integration.after_sha
                        ? ` · ${repository.integration.after_sha.slice(0, 12)}`
                        : ''}
                    </span>
                  ) : null}
                  {repository.merge ? (
                    <span>Merge: {repository.merge.state.replaceAll('_', ' ')}</span>
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
                      {repository.change_state === 'unchanged'
                        ? 'No net changes in the completed repository batch.'
                        : 'Waiting for integration and merge admission.'}
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
      {mergeBatches.length ? (
        <div className="space-y-2 border-t pt-3">
          <h3 className="text-sm font-medium">Repository merge batches</h3>
          <ul className="space-y-2">
            {mergeBatches.map((batch) => (
              <li key={batch.id} className="rounded-md border p-3 text-xs">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant="outline">{batch.state.replaceAll('_', ' ')}</Badge>
                  <span>Revision {batch.repository_set_revision}</span>
                  <span>Version {batch.version}</span>
                </div>
                <ul className="mt-2 space-y-1">
                  {batch.repositories.map((repository) => (
                    <li key={`${batch.id}:${repository.repo_id}`} className="break-all">
                      {repository.repo_id}: {repository.state.replaceAll('_', ' ')}
                      {repository.after_sha ? ` · ${repository.after_sha.slice(0, 12)}` : ''}
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </section>
  )
}
