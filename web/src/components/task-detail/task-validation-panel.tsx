import {
  useProjectValidationChecksQuery,
  useTaskValidationCheckRequestsQuery,
  useTaskValidationComparisonsQuery,
} from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import type { Task } from '@/types/generated'
import { checkRequestStateLabels, comparisonBadge } from './task-verification'

export function TaskValidationPanel({ task }: { task: Task }) {
  const requests = useTaskValidationCheckRequestsQuery(task.id)
  const comparisons = useTaskValidationComparisonsQuery(task.id)
  const catalog = useProjectValidationChecksQuery(task.project_id)
  const definitionById = new Map((catalog.data?.items ?? []).map((item) => [item.id, item]))
  const items = requests.data?.items ?? []
  const comparisonByRequest = new Map(
    (comparisons.data?.items ?? []).map((item) => [item.check_request_id, item]),
  )
  if (!requests.isLoading && !requests.isError && items.length === 0) return null
  return (
    <section className="min-w-0 space-y-3 rounded-lg border p-4" aria-labelledby="validation-checks-title">
      <div>
        <h3 id="validation-checks-title" className="font-semibold">
          Validation checks
        </h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Catalog checks run on an exact commit and, when requested, on the baseline too, so a new
          failure can be told apart from one that was already there.
        </p>
      </div>
      {requests.isLoading ? <p className="text-sm text-muted-foreground">Loading checks…</p> : null}
      {requests.isError ? (
        <div role="alert" className="rounded-md border border-destructive/40 p-3 text-sm text-destructive">
          Validation checks could not be loaded.{' '}
          <Button size="sm" variant="outline" onClick={() => void requests.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      <ul className="space-y-2">
        {items.map((item) => {
          const comparison = comparisonByRequest.get(item.id)
          const definition = definitionById.get(item.check_definition_id)
          const badge = comparison ? comparisonBadge(comparison.classification) : null
          return (
            <li key={item.id} className="min-w-0 space-y-2 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <code className="text-xs font-semibold">
                  {definition ? `${definition.check_key}@${definition.version}` : 'check'}
                </code>
                <Badge variant={item.state === 'failed_to_start' ? 'destructive' : 'outline'}>
                  {checkRequestStateLabels[item.state]}
                </Badge>
                {badge ? <Badge variant={badge.variant}>{badge.label}</Badge> : null}
                <span className="text-xs text-muted-foreground">
                  {item.trigger_kind === 'after_integration' ? 'After integration' : 'Coordinator request'}
                  {item.baseline ? ' · baseline run' : ''}
                </span>
              </div>
              {badge ? <p className="text-muted-foreground">{badge.explanation}</p> : null}
              {comparison ? (
                <p className="text-xs text-muted-foreground">
                  Candidate {comparison.candidate_status} · baseline {comparison.baseline_status}
                  {comparison.baseline_exit_code != null ? ` (exit ${comparison.baseline_exit_code})` : ''}
                  {comparison.same_output_digest ? ' · identical output' : ''}
                </p>
              ) : null}
              {item.failure_code ? (
                <p className="text-xs text-destructive">Failure: {item.failure_code.replaceAll('_', ' ')}</p>
              ) : null}
              {item.reason ? <p className="break-words text-xs text-muted-foreground">{item.reason}</p> : null}
              <details className="text-xs text-muted-foreground">
                <summary className="w-fit cursor-pointer">Exact references</summary>
                <p className="mt-1 break-all">Check definition {item.check_definition_id} (digest {item.definition_digest})</p>
                <p className="break-all">Commit {item.commit_sha}</p>
                {item.candidate_run_id ? (
                  <p className="flex flex-wrap items-center gap-1 break-all">
                    ValidationRun {item.candidate_run_id}
                    <CopyReferenceButton value={item.candidate_run_id} label="ValidationRun ID" />
                  </p>
                ) : null}
                {comparison ? (
                  <p className="break-all">Baseline commit {comparison.baseline_commit_sha}</p>
                ) : null}
              </details>
            </li>
          )
        })}
      </ul>
    </section>
  )
}
