import { Badge } from '@/components/ui/badge'
import {
  useTaskReviewCheckpointsQuery,
  useTaskReviewCoverageQuery,
  useTaskReviewPolicyQuery,
} from '@/api/hooks'
import type { ProjectReviewPolicyResponse } from '@/types/generated'

function shortCommit(commit: string | null | undefined): string {
  return commit ? commit.slice(0, 12) : 'not pinned'
}

function policyLabel(policy: NonNullable<ProjectReviewPolicyResponse['policy']>): string {
  return `${policy.risk_class.toUpperCase()} · ${policy.trigger_mode} · ${policy.coverage_required ? 'coverage required' : 'coverage optional'}`
}

export function TaskReviewCoveragePanel({
  taskId,
  projectPolicy,
}: {
  taskId: string
  projectPolicy?: ProjectReviewPolicyResponse | null
}) {
  const policyQuery = useTaskReviewPolicyQuery(taskId)
  const coverageQuery = useTaskReviewCoverageQuery(taskId)
  const checkpointsQuery = useTaskReviewCheckpointsQuery(taskId)
  const taskPolicy = policyQuery.data?.policy
  const effectivePolicy = taskPolicy?.policy

  return (
    <section className="space-y-3 rounded-lg border p-4" aria-labelledby="review-coverage-title">
      <div>
        <h3 id="review-coverage-title" className="font-semibold">
          Commit-range review coverage
        </h3>
        <p className="mt-1 text-xs text-muted-foreground">
          Coverage follows integrated commits and exact range reports for each writable repository.
        </p>
      </div>

      <div className="rounded-md border p-3 text-sm">
        <h4 className="font-medium">Review policy</h4>
        {policyQuery.isLoading ? (
          <p className="mt-1 text-xs text-muted-foreground">Loading Task policy…</p>
        ) : policyQuery.isError ? (
          <p role="alert" className="mt-1 text-xs text-destructive">
            Task review policy could not be loaded.
          </p>
        ) : effectivePolicy ? (
          <div className="mt-2 space-y-1 text-xs">
            <p>
              Effective Task policy · revision {taskPolicy?.revision} · {taskPolicy?.source.replaceAll('_', ' ')}
            </p>
            <p>{policyLabel(effectivePolicy)}</p>
            <p>Reviewer role: {effectivePolicy.reviewer_role}</p>
          </div>
        ) : (
          <p className="mt-1 text-xs text-muted-foreground">No review policy is pinned to this Task.</p>
        )}
        {projectPolicy?.policy ? (
          <p className="mt-2 text-xs text-muted-foreground">
            Current Project default for new Tasks · version {projectPolicy.version} ·{' '}
            {policyLabel(projectPolicy.policy)}
          </p>
        ) : null}
      </div>

      {coverageQuery.isLoading ? (
        <p className="text-sm text-muted-foreground">Loading repository coverage…</p>
      ) : null}
      {coverageQuery.isError ? (
        <p role="alert" className="text-sm text-destructive">
          Review coverage could not be loaded.
        </p>
      ) : null}
      {(coverageQuery.data?.repositories ?? []).map((repository) => {
        const checkpoints = (checkpointsQuery.data?.items ?? []).filter(
          (checkpoint) => checkpoint.repo_id === repository.repo_id,
        )
        return (
          <article key={repository.repo_id} className="space-y-2 rounded-md border p-3">
            <div className="flex flex-wrap items-center gap-2">
              <h4 className="font-medium">Repository {repository.repo_id}</h4>
              <Badge variant="outline">{repository.status.replaceAll('_', ' ')}</Badge>
              <span className="text-xs text-muted-foreground">
                {repository.unreviewed_commits} unreviewed commit(s)
              </span>
            </div>
            <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
              <span>Open P0: {repository.open_findings.p0}</span>
              <span>Open P1: {repository.open_findings.p1}</span>
              <span>Base: {shortCommit(repository.base_commit)}</span>
              <span>Frontier: {shortCommit(repository.frontier_commit)}</span>
              <span>Candidate: {shortCommit(repository.candidate_commit)}</span>
            </div>
            {repository.reason_code ? (
              <p className="text-xs text-muted-foreground">Coverage reason: {repository.reason_code}</p>
            ) : null}
            <div className="space-y-1 text-xs">
              <p className="font-medium">Reviewed ranges</p>
              {repository.ranges.length === 0 ? (
                <p className="text-muted-foreground">No reviewed range is recorded.</p>
              ) : (
                repository.ranges.map((range) => (
                  <p key={range.range_id} className="break-all text-muted-foreground">
                    {shortCommit(range.base_commit)} → {shortCommit(range.head_commit)} ·{' '}
                    {range.valid_edge ? `${range.reports.length} report(s)` : 'no valid coverage edge'}
                  </p>
                ))
              )}
            </div>
            <div className="space-y-1 text-xs">
              <p className="font-medium">Checkpoints</p>
              {checkpointsQuery.isLoading ? (
                <p className="text-muted-foreground">Loading checkpoints…</p>
              ) : checkpoints.length === 0 ? (
                <p className="text-muted-foreground">No checkpoint is recorded.</p>
              ) : (
                checkpoints.map((checkpoint) => (
                  <p key={checkpoint.id} className="break-all text-muted-foreground">
                    {checkpoint.state.replaceAll('_', ' ')} · attempt {checkpoint.attempt} · head{' '}
                    {shortCommit(checkpoint.head_commit)}
                    {checkpoint.blocked_code ? ` · ${checkpoint.blocked_code}` : ''}
                  </p>
                ))
              )}
              {checkpointsQuery.isError ? (
                <p role="alert" className="text-destructive">
                  Checkpoints could not be loaded.
                </p>
              ) : null}
            </div>
          </article>
        )
      })}
      {!coverageQuery.isLoading && !coverageQuery.isError && coverageQuery.data?.repositories.length === 0 ? (
        <p className="text-sm text-muted-foreground">This Task has no writable repositories.</p>
      ) : null}
    </section>
  )
}
