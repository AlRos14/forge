import type { ReactNode } from 'react'
import {
  useTaskCoordinatorReplansQuery,
  useTaskFallbackReceiptsQuery,
  useTaskMicroFixesQuery,
} from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import type { Task } from '@/types/generated'

function readable(value: string): string {
  return value.replaceAll('_', ' ')
}

function actorLabel(kind: string, id: string): string {
  return `${kind === 'agent' ? 'Agent' : 'Human'} ${id}`
}

function ReceiptSection({
  title,
  loading,
  error,
  empty,
  retry,
  children,
}: {
  title: string
  loading: boolean
  error: boolean
  empty: boolean
  retry: () => void
  children: ReactNode
}) {
  return (
    <section className="space-y-2" aria-label={title}>
      <h4 className="font-medium">{title}</h4>
      {loading ? <p className="text-sm text-muted-foreground">Loading {title.toLowerCase()}…</p> : null}
      {error ? (
        <div role="alert" className="rounded-md border border-destructive/40 p-3 text-sm text-destructive">
          {title} could not be loaded.{' '}
          <Button size="sm" variant="outline" onClick={retry}>Retry</Button>
        </div>
      ) : null}
      {empty && !loading && !error ? (
        <p className="text-sm text-muted-foreground">No {title.toLowerCase()} recorded.</p>
      ) : null}
      {children}
    </section>
  )
}

export function TaskPhaseThreePanel({ task }: { task: Task }) {
  const fallbackQuery = useTaskFallbackReceiptsQuery(task.id)
  const microFixQuery = useTaskMicroFixesQuery(task.id)
  const replanQuery = useTaskCoordinatorReplansQuery(task.id)
  const fallback = fallbackQuery.data?.pages.flatMap((page) => page.items) ?? []
  const microFixes = microFixQuery.data?.pages.flatMap((page) => page.items) ?? []
  const replans = replanQuery.data?.pages.flatMap((page) => page.items) ?? []
  const allEmpty = fallback.length === 0 && microFixes.length === 0 && replans.length === 0
  const noErrors = !fallbackQuery.isError && !microFixQuery.isError && !replanQuery.isError
  const allLoaded = !fallbackQuery.isLoading && !microFixQuery.isLoading && !replanQuery.isLoading

  if (allEmpty && noErrors && allLoaded) return null

  return (
    <section className="min-w-0 space-y-4 rounded-lg border p-4" aria-label="Phase 3 delivery receipts">
      <div>
        <h3 className="font-semibold">Delivery receipts</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Fallback, bounded correction and coordinator replan records for this Task.
        </p>
      </div>

      <ReceiptSection
        title="Fallback"
        loading={fallbackQuery.isLoading}
        error={fallbackQuery.isError}
        empty={fallback.length === 0}
        retry={() => void fallbackQuery.refetch()}
      >
        <ul className="space-y-2">
          {fallback.map((receipt) => (
            <li key={receipt.id} className="space-y-2 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="secondary">{receipt.cost_class}</Badge>
                <Badge variant="outline">{readable(receipt.failure_class)}</Badge>
                <span className="text-xs text-muted-foreground">Chain position {receipt.chain_index + 1}</span>
              </div>
              <p className="break-all">
                {actorLabel(receipt.from_actor_kind, receipt.from_actor_id)} replaced by{' '}
                {actorLabel(receipt.to_actor_kind, receipt.to_actor_id)}
              </p>
              <p className="break-all text-xs text-muted-foreground">
                WorkUnit {receipt.work_unit_id} · origin Execution {receipt.origin_execution_id}
              </p>
            </li>
          ))}
        </ul>
        {fallbackQuery.hasNextPage ? (
          <div className="flex justify-center">
            <Button size="sm" variant="outline" disabled={fallbackQuery.isFetchingNextPage} onClick={() => void fallbackQuery.fetchNextPage()}>
              {fallbackQuery.isFetchingNextPage ? 'Loading…' : 'Load older fallback receipts'}
            </Button>
          </div>
        ) : null}
      </ReceiptSection>

      <ReceiptSection
        title="Micro-fixes"
        loading={microFixQuery.isLoading}
        error={microFixQuery.isError}
        empty={microFixes.length === 0}
        retry={() => void microFixQuery.refetch()}
      >
        <ul className="space-y-2">
          {microFixes.map((receipt) => (
            <li key={receipt.id} className="space-y-2 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant={receipt.bounds_outcome === 'bounds_exceeded' ? 'destructive' : 'secondary'}>
                  {readable(receipt.bounds_outcome)}
                </Badge>
                {receipt.bounds_violation ? <Badge variant="outline">{readable(receipt.bounds_violation)}</Badge> : null}
                {receipt.reviewer_cost_class ? <Badge variant="outline">Reviewer · {receipt.reviewer_cost_class}</Badge> : null}
                <span className="text-xs text-muted-foreground">Micro-fix {receipt.ordinal}</span>
              </div>
              <p className="text-xs text-muted-foreground">
                {receipt.changed_files ?? 'Pending'} files · {receipt.changed_lines ?? 'Pending'} changed lines
              </p>
              <p className="break-all text-xs text-muted-foreground">
                WorkUnit {receipt.fix_work_unit_id} · reviewed commit {receipt.reviewed_commit_sha}
              </p>
              {receipt.declared_paths.length ? (
                <ul className="list-inside list-disc text-xs text-muted-foreground">
                  {receipt.declared_paths.map((path) => <li key={path} className="break-all font-mono">{path}</li>)}
                </ul>
              ) : null}
            </li>
          ))}
        </ul>
        {microFixQuery.hasNextPage ? (
          <div className="flex justify-center">
            <Button size="sm" variant="outline" disabled={microFixQuery.isFetchingNextPage} onClick={() => void microFixQuery.fetchNextPage()}>
              {microFixQuery.isFetchingNextPage ? 'Loading…' : 'Load older micro-fixes'}
            </Button>
          </div>
        ) : null}
      </ReceiptSection>

      <ReceiptSection
        title="Replans"
        loading={replanQuery.isLoading}
        error={replanQuery.isError}
        empty={replans.length === 0}
        retry={() => void replanQuery.refetch()}
      >
        <ul className="space-y-2">
          {replans.map((replan) => (
            <li key={replan.id} className="space-y-2 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">Replan {replan.ordinal}</Badge>
                <Badge variant={replan.classification === 'scope_or_authority_change' ? 'destructive' : 'secondary'}>
                  {replan.classification ? readable(replan.classification) : readable(replan.state)}
                </Badge>
                <Badge variant="outline">{readable(replan.state)}</Badge>
              </div>
              <p className="whitespace-pre-wrap">{replan.reason}</p>
              {replan.classification_facts !== null ? (
                <details>
                  <summary className="w-fit cursor-pointer text-xs text-muted-foreground">Classification facts</summary>
                  <pre className="mt-2 overflow-auto whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs">
                    {JSON.stringify(replan.classification_facts, null, 2)}
                  </pre>
                </details>
              ) : null}
              {replan.human_decision_id ? (
                <a href={`#human-decision-${replan.human_decision_id}`} className="text-primary underline-offset-4 hover:underline">
                  Human decision {replan.human_decision_id.slice(0, 8)}
                </a>
              ) : null}
              {replan.plan_request_id ? (
                <p className="break-all text-xs text-muted-foreground">Plan request {replan.plan_request_id}</p>
              ) : null}
            </li>
          ))}
        </ul>
        {replanQuery.hasNextPage ? (
          <div className="flex justify-center">
            <Button size="sm" variant="outline" disabled={replanQuery.isFetchingNextPage} onClick={() => void replanQuery.fetchNextPage()}>
              {replanQuery.isFetchingNextPage ? 'Loading…' : 'Load older replans'}
            </Button>
          </div>
        ) : null}
      </ReceiptSection>
    </section>
  )
}
