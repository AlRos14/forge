import { Link } from '@tanstack/react-router'
import {
  useTaskInterventionsQuery,
  useTaskQualityRequestsQuery,
  useTaskReworkAutomationQuery,
} from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { resolveActorIdentity } from '@/features/task-roles/ActorIdentity'
import type {
  AgentResponse,
  DaemonResponse,
  GateResponse,
  ProjectMemberResponse,
  ReviewLineResponse,
  Task,
  TaskCoordinationOverviewResponse,
  TaskLaunchPreflightResponse,
  WorkUnitResponse,
} from '@/types/generated'
import {
  deriveJourney,
  type JourneyStage,
  type JourneyStageStatus,
} from './task-journey'
import {
  deriveReviewFindingsProjection,
  isReviewFindingsRequest,
  type ReviewFindingsDataState,
} from './task-review-triage'

const statusLabels: Record<JourneyStageStatus, string> = {
  done: 'Complete',
  current: 'Needs action',
  waiting: 'Waiting',
  blocked: 'Blocked',
  not_started: 'Not started',
}

const statusClasses: Record<JourneyStageStatus, string> = {
  done: 'border-emerald-700/30 bg-emerald-50 text-emerald-950 dark:border-emerald-400/30 dark:bg-emerald-950/20 dark:text-emerald-100',
  current: 'border-primary/40 bg-primary/5',
  waiting: 'border-border-subtle bg-muted/20',
  blocked:
    'border-amber-400/60 bg-amber-50 text-amber-950 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100',
  not_started: 'border-border-subtle bg-card',
}

function StageCard({ stage, index }: { stage: JourneyStage; index: number }) {
  return (
    <li
      className={`min-w-0 rounded-md border p-3 ${statusClasses[stage.status]}`}
      aria-label={`${stage.title}: ${statusLabels[stage.status]}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-current text-xs font-semibold">
          {index + 1}
        </span>
        <h3 className="min-w-0 break-words text-sm font-semibold">{stage.title}</h3>
        <Badge variant={stage.status === 'blocked' ? 'destructive' : 'outline'} className="ml-auto">
          {statusLabels[stage.status]}
        </Badge>
      </div>
      <p className="mt-2 break-words text-sm font-medium">{stage.summary}</p>
      <p className="mt-1 break-words text-xs opacity-80">{stage.detail}</p>
    </li>
  )
}

export function TaskJourneyPanel({
  task,
  coordination,
  dispatch,
  workUnits,
  workUnitsState,
  onRetryWorkUnits,
  reviewLines,
  gates,
  agents,
  members,
  daemons,
  onOpenAdvancedWork,
  onOpenReview,
  onOpenResults,
}: {
  task: Task
  coordination: TaskCoordinationOverviewResponse
  dispatch: TaskLaunchPreflightResponse | undefined
  workUnits: WorkUnitResponse[]
  workUnitsState: ReviewFindingsDataState
  onRetryWorkUnits: () => void
  reviewLines: ReviewLineResponse[]
  gates: GateResponse[]
  agents: AgentResponse[]
  members: ProjectMemberResponse[]
  daemons: DaemonResponse[]
  onOpenAdvancedWork: () => void
  onOpenReview: () => void
  onOpenResults: () => void
}) {
  const qualityQuery = useTaskQualityRequestsQuery(task.id)
  const interventionsQuery = useTaskInterventionsQuery(task.id)
  const reworkAutomationQuery = useTaskReworkAutomationQuery(task.id)
  const automationIntents = reworkAutomationQuery.data?.items ?? []
  const automationIntentsState: ReviewFindingsDataState = reworkAutomationQuery.isError
    ? 'error'
    : reworkAutomationQuery.data === undefined
      ? 'loading'
      : 'loaded'
  const reviewFindings = deriveReviewFindingsProjection({
    overview: coordination,
    request: coordination.latest_request,
    automationIntents,
    automationIntentsState,
    workUnits,
    workUnitsState,
  })
  const journey = deriveJourney({
    task,
    coordination,
    dispatch: dispatch?.dispatch,
    workUnits,
    reviewLines,
    gates,
    qualityRequests: qualityQuery.data?.items ?? [],
    interventions: interventionsQuery.data?.items ?? [],
    reviewAutomationIntents: automationIntents,
    reviewAutomationIntentsState: automationIntentsState,
    workUnitsState,
  })
  const currentRequest = coordination.latest_request
  const requestKindLabel = isReviewFindingsRequest(currentRequest)
    ? 'Review findings'
    : 'WorkUnit plan'
  const blockers = [...new Set([...journey.blockers, ...(reviewFindings?.blockers ?? [])])]

  return (
    <section
      className="min-w-0 space-y-4 rounded-lg border border-primary/30 bg-card p-4 sm:p-5"
      aria-labelledby="task-journey-title"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-xs font-semibold uppercase tracking-wide text-primary">
            Guided Task flow
          </p>
          <h2 id="task-journey-title" className="text-lg font-semibold">
            Objective to delivery
          </h2>
          <p className="mt-1 text-sm text-muted-foreground">
            A projection of durable Task facts. It never treats an Execution count or a local UI
            state as progress.
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" onClick={onOpenReview}>
            Review
          </Button>
          <Button size="sm" variant="outline" onClick={onOpenResults}>
            Results
          </Button>
        </div>
      </div>

      <div
        role="status"
        aria-live="polite"
        className={`rounded-md border p-3 ${journey.escalatedToHuman ? statusClasses.blocked : statusClasses.current}`}
      >
        <p className="text-xs font-medium uppercase tracking-wide opacity-80">
          Who acts now · {journey.nextOwner}
        </p>
        <p className="mt-1 break-words text-sm font-semibold">{journey.nextSummary}</p>
        <p className="mt-1 break-words text-sm opacity-90">{journey.nextDetail}</p>
        {journey.activeActors.length ? (
          <p className="mt-2 break-words text-xs opacity-80">
            Assigned work actors:{' '}
            {journey.activeActors
              .map(
                (actor) =>
                  `${resolveActorIdentity(actor, agents, members, daemons).name} (${actor.kind})`,
              )
              .join(', ')}
            . Exact references are listed in Work and next step.
          </p>
        ) : null}
      </div>

      {currentRequest ? (
        <section
          className="min-w-0 space-y-2 rounded-md border bg-background/50 p-3"
          aria-label="Current coordination request"
        >
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              Current request · {requestKindLabel}
            </p>
            <Badge variant="outline">
              {reviewFindings?.stateLabel ?? currentRequest.state.replaceAll('_', ' ')}
            </Badge>
          </div>
          {reviewFindings ? (
            <>
              {reviewFindings.triage ? (
                <p className="text-sm text-muted-foreground">
                  Coordinator disposition · {reviewFindings.triage.disposition}. The next step is
                  derived from its linked durable continuation records.
                </p>
              ) : null}
              <details className="text-xs text-muted-foreground">
                <summary className="w-fit cursor-pointer">Evidence</summary>
                <div className="mt-2 space-y-1 break-all">
                  <p>Request ID: {currentRequest.id}</p>
                  <p>ReviewReport: {reviewFindings.reportId ?? 'source unavailable'}</p>
                  <p>ReviewReport digest: {reviewFindings.reportDigest ?? 'source unavailable'}</p>
                  <p>
                    Reviewer producer Execution:{' '}
                    {reviewFindings.producerExecutionId ?? 'source unavailable'}
                  </p>
                  {currentRequest.execution_id ? (
                    <p>Coordinator Execution: {currentRequest.execution_id}</p>
                  ) : null}
                  {reviewFindings.triage?.review_work_unit_id ? (
                    <p>Recheck WorkUnit: {reviewFindings.triage.review_work_unit_id}</p>
                  ) : null}
                  {reviewFindings.reworkIntent?.correction_work_unit_id ? (
                    <p>Correction WorkUnit: {reviewFindings.reworkIntent.correction_work_unit_id}</p>
                  ) : null}
                  {reviewFindings.reworkIntent?.review_work_unit_id ? (
                    <p>Follow-up review WorkUnit: {reviewFindings.reworkIntent.review_work_unit_id}</p>
                  ) : null}
                </div>
              </details>
            </>
          ) : (
            <details className="text-xs text-muted-foreground">
              <summary className="w-fit cursor-pointer">Evidence</summary>
              <p className="mt-2 break-all">Request ID: {currentRequest.id}</p>
            </details>
          )}
          {reviewFindings &&
          (automationIntentsState === 'error' || workUnitsState === 'error') ? (
            <Button
              size="sm"
              variant="outline"
              disabled={reworkAutomationQuery.isFetching}
              onClick={() => {
                if (automationIntentsState === 'error') void reworkAutomationQuery.refetch()
                if (workUnitsState === 'error') onRetryWorkUnits()
              }}
            >
              Retry continuation evidence
            </Button>
          ) : null}
        </section>
      ) : null}

      {blockers.length ? (
        <div
          className="rounded-md border border-amber-400/60 bg-amber-50 p-3 text-sm text-amber-950 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100"
          aria-labelledby="journey-blockers-title"
        >
          <h3 id="journey-blockers-title" className="font-medium">
            Why Forge is stopped or waiting
          </h3>
          <ul className="mt-1 list-disc space-y-1 pl-5">
            {blockers.slice(0, 4).map((blocker) => (
              <li key={blocker} className="break-words">
                {blocker}
              </li>
            ))}
          </ul>
          {blockers.length > 4 ? (
            <p className="mt-1 text-xs opacity-80">
              {blockers.length - 4} more blocker
              {blockers.length - 4 === 1 ? '' : 's'} are listed in the panels below.
            </p>
          ) : null}
        </div>
      ) : null}

      <ol className="grid min-w-0 gap-2 sm:grid-cols-2 xl:grid-cols-5" aria-label="Task stages">
        {journey.stages.map((stage, index) => (
          <StageCard key={stage.id} stage={stage} index={index} />
        ))}
      </ol>

      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <Button size="sm" variant="outline" onClick={onOpenAdvancedWork}>
          Advanced work
        </Button>
        <Link
          to="/projects/$projectId/settings/$tab"
          params={{ projectId: task.project_id, tab: 'workers' }}
          className="underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          Project team &amp; templates
        </Link>
        <span>·</span>
        <span>Lifecycle: {task.lifecycle.state.replaceAll('_', ' ')}</span>
      </div>
    </section>
  )
}
