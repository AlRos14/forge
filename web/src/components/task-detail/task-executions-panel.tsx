import { type Dispatch, type SetStateAction, useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { ArrowsClockwise, CaretDown, CaretRight } from '@phosphor-icons/react'
import { toast } from 'sonner'
import { useApproveReview, useRejectReview, useTransitionTask, useTriggerReview } from '@/api/hooks'
import { ChatEntryContainer } from '@/components/chat'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import { cn } from '@/lib/cn'
import { buildExecutionChains, roleDisplayName, turnLabel } from '@/lib/execution-utils'
import { productTerm } from '@/lib/i18n'
import type { Execution, Review, Task } from '@/types/generated'

const executionStatusColors: Record<Execution['status'], string> = {
  running: 'bg-sky-100 text-sky-900 dark:bg-sky-950 dark:text-sky-300',
  completed: 'bg-emerald-100 text-emerald-900 dark:bg-emerald-950 dark:text-emerald-300',
  failed: 'bg-red-100 text-red-900 dark:bg-red-950 dark:text-red-300',
  cancelled: 'bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-400',
}

export const reviewStatusColors: Record<Review['status'], string> = {
  running: 'bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300',
  awaiting_human: 'bg-violet-100 text-violet-900 dark:bg-violet-950 dark:text-violet-300',
  passed: 'bg-emerald-100 text-emerald-900 dark:bg-emerald-950 dark:text-emerald-300',
  failed: 'bg-red-100 text-red-900 dark:bg-red-950 dark:text-red-300',
  cancelled: 'bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-400',
}

function stepOutput(step: Review['step_results'][number]) {
  return step.output_tail?.trim() ? step.output_tail : step.stderr_tail
}

interface TaskExecutionsPanelProps {
  taskId: string
  executions: Execution[]
  isLoading: boolean
  agentName: (agentId?: string | null) => string | undefined | null
  formatDate: (value?: string | null) => string
  onClose: () => void
}

const VISIBLE_TURNS = 3

export function TaskExecutionsPanel({
  taskId,
  executions,
  isLoading,
  agentName,
  formatDate,
  onClose,
}: TaskExecutionsPanelProps) {
  const navigate = useNavigate()
  const [expandedChains, setExpandedChains] = useState<Set<string>>(new Set())

  if (isLoading) {
    return (
      <div className="space-y-2">
        <Skeleton className="h-16 w-full" />
        <Skeleton className="h-16 w-full" />
      </div>
    )
  }

  if (executions.length === 0) {
    return (
      <div className="rounded-lg border border-dashed p-8 text-center text-sm text-muted-foreground">
        No {productTerm('run', 0).toLowerCase()} yet
      </div>
    )
  }

  const chains = buildExecutionChains(executions)

  return (
    <div className="space-y-2.5">
      {chains.map((chain) => {
        const totalTurns = chain.turns.length
        const lastTurn = chain.turns[totalTurns - 1]
        const sessionAgent = agentName(chain.root.agent_id)
        const isRunning = lastTurn.status === 'running'
        const reversedTurns = [...chain.turns].reverse()
        const isExpanded = expandedChains.has(chain.root.id)
        const hiddenCount = totalTurns - VISIBLE_TURNS
        const visibleTurns = isExpanded || hiddenCount <= 0
          ? reversedTurns
          : reversedTurns.slice(0, VISIBLE_TURNS)

        return (
          <div
            key={chain.root.id}
            className={cn(
              'rounded-lg border overflow-hidden',
              isRunning && 'border-sky-200 dark:border-sky-900',
            )}
          >
            {/* Session header */}
            <div
              className={cn(
                'flex items-center justify-between gap-2 px-3 py-2',
                isRunning ? 'bg-sky-50/60 dark:bg-sky-950/20' : 'bg-muted/40',
              )}
            >
              <div className="flex items-center gap-2 min-w-0">
                <span className="text-xs font-semibold">
                  {roleDisplayName(chain.root.role)} Session
                </span>
                {sessionAgent && (
                  <span className="text-[11px] text-muted-foreground truncate">{sessionAgent}</span>
                )}
                {totalTurns > 1 && (
                  <span className="text-[11px] text-muted-foreground/60">
                    · {totalTurns} turns
                  </span>
                )}
              </div>
              <Badge
                className={cn(
                  'shrink-0 border-transparent text-[11px]',
                  executionStatusColors[lastTurn.status],
                )}
              >
                {lastTurn.status}
              </Badge>
            </div>

            {/* Turn rows — latest first, capped at VISIBLE_TURNS */}
            <div className="divide-y">
              {visibleTurns.map((execution, displayIndex) => {
                const originalIndex = totalTurns - 1 - displayIndex
                return (
                  <button
                    key={execution.id}
                    className="flex w-full items-center gap-2.5 px-3 py-2 text-left transition-colors hover:bg-accent cursor-pointer"
                    type="button"
                    onClick={() => {
                      onClose()
                      void navigate({
                        to: '/tasks/$taskId/executions/$executionId',
                        params: { taskId, executionId: execution.id },
                      })
                    }}
                  >
                    {totalTurns > 1 && (
                      <span className="w-6 shrink-0 text-[11px] font-mono text-muted-foreground/40 text-right">
                        T{originalIndex + 1}
                      </span>
                    )}
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="text-xs text-muted-foreground">
                          {turnLabel(originalIndex, execution)}
                        </span>
                        <Badge
                          className={cn(
                            'border-transparent text-[11px]',
                            executionStatusColors[execution.status],
                          )}
                        >
                          {execution.status}
                        </Badge>
                        <span className="text-[11px] text-muted-foreground/50">
                          {formatDate(execution.created_at)}
                        </span>
                      </div>
                    </div>
                    <CaretRight size={13} className="shrink-0 text-muted-foreground/40" />
                  </button>
                )
              })}

              {/* Expand button for older turns */}
              {!isExpanded && hiddenCount > 0 && (
                <button
                  type="button"
                  className="flex w-full items-center justify-center gap-1.5 px-3 py-1.5 text-[11px] text-muted-foreground hover:text-foreground hover:bg-accent transition-colors cursor-pointer"
                  onClick={() => setExpandedChains((prev) => new Set(prev).add(chain.root.id))}
                >
                  <CaretDown size={11} />
                  <span>{hiddenCount} older {hiddenCount === 1 ? 'turn' : 'turns'}</span>
                </button>
              )}
            </div>
          </div>
        )
      })}
    </div>
  )
}

interface TaskReviewPanelProps {
  task: Task
  latestReview?: Review
  reviews: Review[]
  reviewsLoading: boolean
  expandedHistoryAttempts: Set<number>
  rejectReason: string
  showRejectInput: boolean
  triggerReview: ReturnType<typeof useTriggerReview>
  approveReview: ReturnType<typeof useApproveReview>
  rejectReview: ReturnType<typeof useRejectReview>
  transitionTask: ReturnType<typeof useTransitionTask>
  setRejectReason: Dispatch<SetStateAction<string>>
  setShowRejectInput: Dispatch<SetStateAction<boolean>>
  rerunReview: () => void
  onStatusChange: (status: string) => void
  onToggleHistoryAttempt: (attemptNumber: number) => void
  formatDate: (value?: string | null) => string
}

export function TaskReviewPanel({
  task,
  latestReview,
  reviews,
  reviewsLoading,
  expandedHistoryAttempts,
  rejectReason,
  showRejectInput,
  triggerReview,
  approveReview,
  rejectReview,
  transitionTask,
  setRejectReason,
  setShowRejectInput,
  rerunReview,
  onStatusChange,
  onToggleHistoryAttempt,
  formatDate,
}: TaskReviewPanelProps) {
  return (
    <>
      {task.status === 'review' || (latestReview && latestReview.attempt_number > 0) ? (
        <div className="rounded-lg border p-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="space-y-1">
              <p className="text-sm font-medium">Attempt {latestReview?.attempt_number ?? 1}</p>
              {latestReview ? (
                <div className="flex flex-wrap items-center gap-2">
                  <Badge
                    className={cn(
                      'border-transparent text-[11px]',
                      reviewStatusColors[latestReview.status],
                      latestReview.status === 'running' && 'animate-pulse',
                    )}
                  >
                    {latestReview.status}
                  </Badge>
                  <span className="text-xs text-muted-foreground">
                    {formatDate(latestReview.started_at)} - {formatDate(latestReview.finished_at)}
                  </span>
                </div>
              ) : (
                <p className="text-xs text-muted-foreground">No review details yet.</p>
              )}
            </div>
            <Button
              size="sm"
              variant="outline"
              disabled={task.status !== 'review' || triggerReview.isPending}
              onClick={rerunReview}
            >
              <ArrowsClockwise
                size={14}
                className={cn(triggerReview.isPending && 'animate-spin')}
              />
              <span className="ml-1.5">{triggerReview.isPending ? 'Running...' : 'Re-run'}</span>
            </Button>
          </div>
        </div>
      ) : null}

      {task.status === 'review' && latestReview?.status === 'awaiting_human' ? (
        <div className="rounded-lg border border-violet-200 bg-violet-50 p-4 dark:border-violet-900 dark:bg-violet-950/30">
          <p className="text-sm font-medium">Manual Review Required</p>
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              disabled={approveReview.isPending}
              onClick={() => {
                approveReview.mutate(task.id, {
                  onError: (error) => toast.error(getApiErrorMessage(error, 'Approve failed')),
                })
              }}
            >
              Approve
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={rejectReview.isPending}
              onClick={() => setShowRejectInput((value) => !value)}
            >
              Reject
            </Button>
          </div>
          {showRejectInput ? (
            <div className="mt-3 space-y-2">
              <Textarea
                placeholder="Reason for rejection"
                value={rejectReason}
                onChange={(event) => setRejectReason(event.target.value)}
              />
              <Button
                size="sm"
                variant="outline"
                disabled={rejectReview.isPending}
                onClick={() => {
                  rejectReview.mutate(
                    {
                      taskId: task.id,
                      reason: rejectReason.trim() || undefined,
                    },
                    {
                      onSuccess: () => {
                        setRejectReason('')
                        setShowRejectInput(false)
                      },
                      onError: (error) => toast.error(getApiErrorMessage(error, 'Reject failed')),
                    },
                  )
                }}
              >
                Confirm Reject
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}

      {task.status === 'review' && latestReview?.status === 'passed' ? (
        <div className="flex gap-2">
          <Button
            size="sm"
            disabled={transitionTask.isPending}
            onClick={() => onStatusChange('merging')}
          >
            Merge
          </Button>
        </div>
      ) : null}
      {task.status === 'review' && latestReview?.status === 'failed' ? (
        <Button
          size="sm"
          variant="outline"
          disabled={transitionTask.isPending}
          onClick={() => onStatusChange('in_progress')}
        >
          Back to In Progress
        </Button>
      ) : null}

      <div className="rounded-lg border p-4">
        <div className="flex items-center justify-between">
          <p className="text-sm font-medium">Step results</p>
          {latestReview && latestReview.step_results.length > 0 ? (
            <Badge
              className={cn(
                'border-transparent text-[11px]',
                latestReview.step_results.every((step) => step.exit_code === 0)
                  ? 'bg-emerald-100 text-emerald-900 dark:bg-emerald-950 dark:text-emerald-300'
                  : 'bg-red-100 text-red-900 dark:bg-red-950 dark:text-red-300',
              )}
            >
              {latestReview.step_results.length} steps{' '}
              {latestReview.step_results.every((step) => step.exit_code === 0)
                ? 'passed'
                : 'finished'}
            </Badge>
          ) : latestReview?.step_results.length === 0 && latestReview.status === 'passed' ? (
            <Badge className="border-transparent bg-emerald-100 text-emerald-900 dark:bg-emerald-950 dark:text-emerald-300 text-[11px]">
              Auto-passed
            </Badge>
          ) : null}
        </div>
        <div className="mt-3 space-y-2">
          {latestReview ? (
            latestReview.step_results.length > 0 ? (
              <>
                <p className="text-xs text-muted-foreground">
                  Successful steps are collapsed. Expand a step to view captured output.
                </p>
                {latestReview.step_results.map((step) => (
                  <ChatEntryContainer
                    key={step.index}
                    variant="tool"
                    status={step.exit_code === 0 ? 'success' : 'failed'}
                    header={
                      <span className="flex items-center gap-2">
                        <span className="font-mono text-xs">{step.command}</span>
                        <Badge
                          className={cn(
                            'border-transparent text-micro',
                            step.exit_code === 0
                              ? 'bg-emerald-100 text-emerald-900'
                              : 'bg-red-100 text-red-900',
                          )}
                        >
                          exit {step.exit_code}
                        </Badge>
                      </span>
                    }
                    defaultCollapsed={step.exit_code === 0}
                  >
                    {stepOutput(step) ? (
                      <pre className="whitespace-pre-wrap break-all rounded-md bg-muted p-2 text-xs font-mono">
                        {stepOutput(step)}
                      </pre>
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        No output captured for this step.
                      </p>
                    )}
                  </ChatEntryContainer>
                ))}
              </>
            ) : latestReview.status === 'passed' ? null : (
              <p className="text-sm text-muted-foreground">No step results available.</p>
            )
          ) : reviewsLoading ? (
            <Skeleton className="h-16 w-full" />
          ) : (
            <p className="text-sm text-muted-foreground">No review data available.</p>
          )}
        </div>
      </div>

      {reviews.length > 0 ? (
        <details className="rounded-lg border p-4">
          <summary className="cursor-pointer text-sm font-medium">
            History ({reviews.length})
          </summary>
          <div className="mt-3 space-y-2">
            {reviews
              .slice()
              .sort((a, b) => b.attempt_number - a.attempt_number)
              .map((review) => {
                const isExpanded = expandedHistoryAttempts.has(review.attempt_number)
                return (
                  <div key={review.id} className="rounded-md border p-2">
                    <button
                      className="flex w-full flex-wrap items-center gap-2 text-left"
                      type="button"
                      onClick={() => onToggleHistoryAttempt(review.attempt_number)}
                    >
                      <Badge variant="outline" className="text-[11px]">
                        #{review.attempt_number}
                      </Badge>
                      <Badge
                        className={cn(
                          'border-transparent text-[11px]',
                          reviewStatusColors[review.status],
                        )}
                      >
                        {review.status}
                      </Badge>
                      <span className="text-xs text-muted-foreground">
                        {formatDate(review.started_at)}
                      </span>
                    </button>
                    {isExpanded ? (
                      <div className="mt-2 space-y-1 text-xs">
                        {review.step_results.length > 0 ? (
                          review.step_results.map((step) => (
                            <ChatEntryContainer
                              key={`${review.id}-${step.index}`}
                              variant="tool"
                              status={step.exit_code === 0 ? 'success' : 'failed'}
                              header={
                                <span className="flex items-center gap-2">
                                  <span className="font-mono text-xs">{step.command}</span>
                                  <Badge
                                    className={cn(
                                      'border-transparent text-micro',
                                      step.exit_code === 0
                                        ? 'bg-emerald-100 text-emerald-900'
                                        : 'bg-red-100 text-red-900',
                                    )}
                                  >
                                    exit {step.exit_code}
                                  </Badge>
                                </span>
                              }
                              defaultCollapsed={step.exit_code === 0}
                            >
                              {stepOutput(step) ? (
                                <pre className="whitespace-pre-wrap break-all rounded-md bg-muted p-2 text-xs font-mono">
                                  {stepOutput(step)}
                                </pre>
                              ) : (
                                <p className="text-xs text-muted-foreground">No output</p>
                              )}
                            </ChatEntryContainer>
                          ))
                        ) : (
                          <p className="text-muted-foreground">No step results.</p>
                        )}
                      </div>
                    ) : null}
                  </div>
                )
              })}
          </div>
        </details>
      ) : null}
    </>
  )
}
