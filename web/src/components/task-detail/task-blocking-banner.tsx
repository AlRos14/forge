import type { InterruptionMetadata, RecoveryAction, Task } from '@/types/generated'
import { CaretDown } from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  getBlockingAnnotation,
  getStaleBlockingAnnotation,
  getTaskWorkflowWarning,
} from '@/lib/workflow-utils'

type RecoveryActionLabelMap = Record<RecoveryAction, string>

type HookFailureDetails = {
  hook?: {
    command?: string
    exit_code?: number | null
    timeout?: boolean
    duration_ms?: number
    working_dir?: string
    stdout?: string
    stderr?: string
    log_path?: string | null
  }
}

const RECOVERY_ACTION_LABELS: RecoveryActionLabelMap = {
  resume_session: 'Resume',
  reexecute: 'Re-execute',
  reset_to_initial: 'Reset to Start',
  cancel_task: 'Cancel Task',
  mark_reviewed: 'Mark Reviewed',
  retry_hook: 'Retry Hook',
  resume_process: 'Resume Process',
  update_workspace_and_retry_hook: 'Update Workspace + Retry',
  skip_hook_once: 'Skip Hook Once',
  reset_retry_window: 'Reset Budget + Retry',
  proceed_once: 'Proceed Once',
  open_interactive: 'Open Interactive',
}

const RECOVERY_ACTION_TITLES: RecoveryActionLabelMap = {
  resume_session: 'Continues this workflow step and may move the task when it finishes',
  reexecute: 'Re-runs this workflow step and may move the task if still in the matching state',
  reset_to_initial: 'Resets task to its initial workflow state',
  cancel_task: 'Permanently cancels this task',
  mark_reviewed: 'Marks the review as passed manually',
  retry_hook: 'Retries the failed hook',
  resume_process: 'Sends a failed review back to implementation',
  update_workspace_and_retry_hook:
    'Rebases the task workspace onto the latest target branch, then retries the failed hook',
  skip_hook_once: 'Skips this hook and proceeds',
  reset_retry_window: 'Starts a fresh retry window for this workflow gate',
  proceed_once: 'Bypasses this guard for one recovery action',
  open_interactive: 'Opens the existing manual or session follow-up flow',
}

const RETRY_FAMILY: RecoveryAction[] = [
  'resume_process',
  'retry_hook',
  'update_workspace_and_retry_hook',
  'reexecute',
  'resume_session',
  'reset_retry_window',
  'skip_hook_once',
]

const RETRY_PRIMARY_ORDER: RecoveryAction[] = [
  'resume_process',
  'retry_hook',
  'reexecute',
  'resume_session',
  'update_workspace_and_retry_hook',
  'reset_retry_window',
  'skip_hook_once',
]

function recoveryActionLabel(task: Task, action: RecoveryAction): string {
  const annotation = getBlockingAnnotation(task)
  if (action === 'retry_hook' && annotation?.blocking_reason === 'target_repo_dirty') {
    return 'Resume Merge'
  }
  return RECOVERY_ACTION_LABELS[action] ?? action
}

interface TaskBlockingBannerProps {
  task: Task
  disabled?: boolean
  onRecover: (action: RecoveryAction) => void
  onCancelTask?: () => void
  cancelPending?: boolean
}

function humanizeBlockingReason(reason: string) {
  const withSpaces = reason.replace(/_/g, ' ')
  return withSpaces.charAt(0).toUpperCase() + withSpaces.slice(1)
}

function InterruptionBanner({
  title,
  metadata,
  tone,
}: {
  title: string
  metadata: InterruptionMetadata
  tone: 'blocked' | 'failed'
}) {
  const classes =
    tone === 'blocked'
      ? 'border-red-300 bg-red-50 text-red-900 dark:border-red-800 dark:bg-red-950 dark:text-red-200'
      : 'border-amber-300 bg-amber-50 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200'

  return (
    <section className={`rounded-lg border p-4 ${classes}`}>
      <div className="space-y-1.5">
        <p className="text-sm font-semibold">{title}</p>
        <p className="text-sm">{metadata.reason}</p>
        {metadata.kind || metadata.source || metadata.execution_id ? (
          <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs opacity-80">
            {metadata.kind ? <span>{humanizeBlockingReason(metadata.kind)}</span> : null}
            {metadata.source ? <span>{metadata.source}</span> : null}
            {metadata.execution_id ? (
              <span className="font-mono">{metadata.execution_id}</span>
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  )
}

export function TaskBlockingBanner({
  task,
  disabled = false,
  onRecover,
  onCancelTask,
  cancelPending = false,
}: TaskBlockingBannerProps) {
  const annotation = getBlockingAnnotation(task)
  const staleAnnotation = getStaleBlockingAnnotation(task)
  const workflowWarning = getTaskWorkflowWarning(task)
  if (task.status === 'cancelled') return null

  if (task.failed) {
    return <InterruptionBanner title="Task Failed" metadata={task.failed} tone="failed" />
  }

  if (task.blocked && !annotation) {
    return <InterruptionBanner title="Task Blocked" metadata={task.blocked} tone="blocked" />
  }

  if (staleAnnotation) {
    return (
      <section className="rounded-lg border border-amber-300 bg-amber-50 p-4 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200">
        <div className="space-y-1.5">
          <p className="text-sm font-semibold">Previous Execution Warning</p>
          <p className="text-sm">Superseded by a later execution or manual state change.</p>
          {staleAnnotation.message ? <p className="text-sm">{staleAnnotation.message}</p> : null}
          {staleAnnotation.blocked_execution_id ? (
            <p className="break-all font-mono text-xs opacity-80">
              {staleAnnotation.blocked_execution_id}
            </p>
          ) : null}
        </div>
      </section>
    )
  }

  if (workflowWarning) {
    return (
      <section className="rounded-lg border border-amber-300 bg-amber-50 p-4 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200">
        <div className="space-y-1.5">
          <p className="text-sm font-semibold">{workflowWarning.title}</p>
          <p className="text-sm">{workflowWarning.message}</p>
        </div>
      </section>
    )
  }

  if (!annotation) return null

  const title = task.blocked?.reason?.trim()
    ? task.blocked.reason
    : annotation.blocking_reason?.trim()
      ? humanizeBlockingReason(annotation.blocking_reason)
      : 'Task Paused'

  const REASON_REQUIRED_ACTIONS: RecoveryAction[] = ['proceed_once']
  const allActions = (
    Array.isArray(annotation.recovery_actions) ? annotation.recovery_actions : []
  ).filter((a) => !REASON_REQUIRED_ACTIONS.includes(a))

  const retryActions = allActions.filter((a) => RETRY_FAMILY.includes(a))
  const primaryRetry =
    RETRY_PRIMARY_ORDER.map((k) => retryActions.find((a) => a === k)).find(Boolean) ??
    retryActions[0] ??
    null
  const secondaryRetries = retryActions.filter((a) => a !== primaryRetry)
  const openInteractive = allActions.find((a) => a === 'open_interactive') ?? null
  const standaloneActions = allActions.filter(
    (a) => !RETRY_FAMILY.includes(a) && a !== 'open_interactive',
  )

  const hook = (annotation as typeof annotation & HookFailureDetails).hook

  return (
    <section className="rounded-lg border border-red-300 bg-red-50 p-4 dark:border-red-800 dark:bg-red-950">
      <div className="space-y-2">
        <p className="text-sm font-medium text-red-900 dark:text-red-200">{title}</p>
        {annotation.message ? (
          <p className="text-sm text-red-800 dark:text-red-300">{annotation.message}</p>
        ) : null}
        {hook ? (
          <div className="space-y-2 rounded-md border border-red-200 bg-white/70 p-3 text-xs dark:border-red-800 dark:bg-black/20">
            {hook.command ? (
              <p className="break-words font-mono text-red-950 dark:text-red-100">{hook.command}</p>
            ) : null}
            <div className="flex flex-wrap gap-x-3 gap-y-1 text-red-800 dark:text-red-200">
              {typeof hook.exit_code === 'number' ? <span>exit {hook.exit_code}</span> : null}
              {hook.timeout ? <span>timeout</span> : null}
              {typeof hook.duration_ms === 'number' ? <span>{hook.duration_ms}ms</span> : null}
            </div>
            {hook.working_dir ? (
              <p className="break-all font-mono text-[11px] text-red-700 dark:text-red-300">
                {hook.working_dir}
              </p>
            ) : null}
            {hook.stderr ? (
              <pre className="max-h-28 overflow-auto whitespace-pre-wrap rounded bg-red-500/10 p-2 font-mono text-[11px] text-red-700 dark:text-red-300">
                {hook.stderr}
              </pre>
            ) : null}
            {hook.stdout ? (
              <pre className="max-h-28 overflow-auto whitespace-pre-wrap rounded bg-red-100/80 p-2 font-mono text-[11px] text-red-950 dark:bg-red-950/50 dark:text-red-100">
                {hook.stdout}
              </pre>
            ) : null}
          </div>
        ) : null}

        {allActions.length > 0 || onCancelTask ? (
          <div className="flex flex-wrap items-center gap-2">
            {primaryRetry ? (
              <div className="flex items-center">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={disabled}
                  title={RECOVERY_ACTION_TITLES[primaryRetry]}
                  className={secondaryRetries.length > 0 ? 'rounded-r-none border-r-0' : ''}
                  onClick={() => onRecover(primaryRetry)}
                >
                  {recoveryActionLabel(task, primaryRetry)}
                </Button>
                {secondaryRetries.length > 0 ? (
                  <DropdownMenu>
                    <DropdownMenuTrigger
                      aria-label="More recovery actions"
                      disabled={disabled}
                      className="inline-flex h-7 w-6 cursor-pointer items-center justify-center rounded-l-none rounded-r-md border border-input bg-card text-secondary-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
                    >
                      <CaretDown className="h-3 w-3" weight="bold" />
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      {secondaryRetries.map((action) => (
                        <DropdownMenuItem key={action} onClick={() => onRecover(action)}>
                          {recoveryActionLabel(task, action)}
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuContent>
                  </DropdownMenu>
                ) : null}
              </div>
            ) : null}
            {openInteractive ? (
              <Button
                size="sm"
                variant="outline"
                disabled={disabled}
                title={RECOVERY_ACTION_TITLES.open_interactive}
                onClick={() => onRecover(openInteractive)}
              >
                Open Interactive
              </Button>
            ) : null}
            {standaloneActions.map((action) => (
              <Button
                key={action}
                size="sm"
                variant="outline"
                disabled={disabled}
                title={RECOVERY_ACTION_TITLES[action]}
                onClick={() => onRecover(action)}
              >
                {recoveryActionLabel(task, action)}
              </Button>
            ))}
            {onCancelTask ? (
              <Button
                size="sm"
                variant="outline"
                disabled={cancelPending}
                className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                onClick={onCancelTask}
              >
                Cancel Task
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  )
}
