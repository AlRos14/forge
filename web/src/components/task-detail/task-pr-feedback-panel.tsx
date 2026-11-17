import { Link } from '@tanstack/react-router'
import { useTaskPrFeedbackQuery, useTaskPrReworkQuery } from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import type { Task } from '@/types/generated'

function readable(value: string): string {
  return value.replaceAll('_', ' ')
}

function FeedbackHeadBadge({ state }: { state: string }) {
  const current = state === 'current'
  return <Badge variant={current ? 'secondary' : 'outline'}>{current ? 'Current head' : readable(state)}</Badge>
}

export function TaskPrFeedbackPanel({ task }: { task: Task }) {
  const feedbackQuery = useTaskPrFeedbackQuery(task.id)
  const reworkQuery = useTaskPrReworkQuery(task.id)
  const feedback = feedbackQuery.data?.pages.flatMap((page) => page.items) ?? []
  const rework = reworkQuery.data?.pages.flatMap((page) => page.items) ?? []
  const hasContent = feedback.length > 0 || rework.length > 0

  if (
    !feedbackQuery.isLoading &&
    !feedbackQuery.isError &&
    !reworkQuery.isLoading &&
    !reworkQuery.isError &&
    !hasContent
  ) {
    return null
  }

  return (
    <section className="min-w-0 space-y-4 rounded-lg border p-4" aria-labelledby="pr-feedback-title">
      <div>
        <h3 id="pr-feedback-title" className="font-semibold">PR feedback</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Feedback items show their trust and head state, triage disposition, and any linked follow-up.
        </p>
      </div>
      {feedbackQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading PR feedback…</p> : null}
      {feedbackQuery.isError ? (
        <div role="alert" className="rounded-md border border-destructive/40 p-3 text-sm text-destructive">
          PR feedback could not be loaded.{' '}
          <Button size="sm" variant="outline" onClick={() => void feedbackQuery.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      <ul className="space-y-2">
        {feedback.map((item) => (
          <li key={item.id} className="min-w-0 space-y-2 rounded-md border p-3 text-sm">
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="outline">{readable(item.trust_class)}</Badge>
              <FeedbackHeadBadge state={item.head_state} />
              <Badge variant={item.disposition ? 'secondary' : 'outline'}>
                {item.disposition ? readable(item.disposition) : 'Untriaged'}
              </Badge>
              <span className="text-xs text-muted-foreground">
                {item.kind.replaceAll('_', ' ')} · @{item.author_login}
              </span>
            </div>
            {item.path ? (
              <p className="break-all font-mono text-xs text-muted-foreground">
                {item.path}{item.line == null ? '' : `:${item.line}`}
              </p>
            ) : null}
            <details>
              <summary className="w-fit cursor-pointer text-xs text-muted-foreground">
                Feedback body{item.body_truncated ? ' (truncated)' : ''}
              </summary>
              <p className="mt-2 whitespace-pre-wrap break-words text-sm">{item.body}</p>
            </details>
            {item.dismiss_reason ? (
              <p className="text-xs text-muted-foreground">Dismiss reason: {readable(item.dismiss_reason)}</p>
            ) : null}
            {item.scope_classification ? (
              <p className="text-xs text-muted-foreground">
                Scope: {readable(item.scope_classification)}
              </p>
            ) : null}
            <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
              {item.followup_task_id ? (
                <Link
                  to="/tasks/$taskId"
                  params={{ taskId: item.followup_task_id }}
                  className="text-primary underline-offset-4 hover:underline"
                >
                  Follow-up Task {item.followup_task_id.slice(0, 8)}
                </Link>
              ) : null}
              {item.human_decision_id ? (
                <a
                  href={`#human-decision-${item.human_decision_id}`}
                  className="text-primary underline-offset-4 hover:underline"
                >
                  Human decision {item.human_decision_id.slice(0, 8)}
                </a>
              ) : null}
              {item.provider_url ? (
                <a
                  href={item.provider_url}
                  target="_blank"
                  rel="noreferrer"
                  className="text-primary underline-offset-4 hover:underline"
                >
                  Open on provider
                </a>
              ) : null}
            </div>
          </li>
        ))}
      </ul>
      {feedbackQuery.hasNextPage ? (
        <div className="flex justify-center">
          <Button
            size="sm"
            variant="outline"
            disabled={feedbackQuery.isFetchingNextPage}
            onClick={() => void feedbackQuery.fetchNextPage()}
          >
            {feedbackQuery.isFetchingNextPage ? 'Loading…' : 'Load older feedback'}
          </Button>
        </div>
      ) : null}

      <div className="space-y-3 border-t pt-4">
        <div>
          <h4 className="font-medium">PR rework status</h4>
          <p className="mt-1 text-sm text-muted-foreground">
            Rework pauses and resumes admission against the same pull request.
          </p>
        </div>
        {reworkQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading rework status…</p> : null}
        {reworkQuery.isError ? (
          <div role="alert" className="rounded-md border border-destructive/40 p-3 text-sm text-destructive">
            PR rework status could not be loaded.{' '}
            <Button size="sm" variant="outline" onClick={() => void reworkQuery.refetch()}>
              Retry
            </Button>
          </div>
        ) : null}
        <ul className="space-y-2">
          {rework.map((item) => (
            <li key={item.id} className="space-y-2 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{readable(item.state)}</Badge>
                <span className="text-xs text-muted-foreground">PR {item.provider_pr_id}</span>
              </div>
              <p className="break-all text-xs text-muted-foreground">
                Superseded head: {item.superseded_head_sha}
              </p>
              {item.convergence_decision_id ? (
                <p className="break-all text-xs text-muted-foreground">
                  Convergence decision: {item.convergence_decision_id}
                </p>
              ) : null}
            </li>
          ))}
        </ul>
        {reworkQuery.hasNextPage ? (
          <div className="flex justify-center">
            <Button
              size="sm"
              variant="outline"
              disabled={reworkQuery.isFetchingNextPage}
              onClick={() => void reworkQuery.fetchNextPage()}
            >
              {reworkQuery.isFetchingNextPage ? 'Loading…' : 'Load older rework'}
            </Button>
          </div>
        ) : null}
      </div>
    </section>
  )
}
