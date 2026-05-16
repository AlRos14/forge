import { Link } from '@tanstack/react-router'
import { CaretDown } from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip } from '@/components/ui/tooltip'
import { workflowLabelFromKind } from '@/components/workflow-health-badge'
import type {
  RecoveryAction,
  Task,
  WorkflowExceptionAction,
  WorkflowExceptionSummary,
} from '@/types/generated'

const EXCEPTION_RETRY_FAMILY: RecoveryAction[] = [
  'resume_process',
  'retry_hook',
  'update_workspace_and_retry_hook',
  'reexecute',
  'resume_session',
  'reset_retry_window',
  'skip_hook_once',
  'proceed_once',
]

const EXCEPTION_RETRY_PRIMARY_ORDER: RecoveryAction[] = [
  'resume_process',
  'retry_hook',
  'reexecute',
  'resume_session',
  'update_workspace_and_retry_hook',
  'reset_retry_window',
  'skip_hook_once',
  'proceed_once',
]

function actionKey(action: WorkflowExceptionAction, index: number) {
  return `${action.kind}:${action.target_execution_id ?? ''}:${index}`
}

function FailingStepDetails({ exception }: { exception: WorkflowExceptionSummary }) {
  const step = exception.failing_step
  if (!step) return null

  return (
    <div className="space-y-2 rounded-md border border-amber-200 bg-white/70 p-3 text-xs dark:border-amber-800 dark:bg-black/20">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <p className="font-medium">Failing step</p>
        <span>step {step.index}</span>
        {typeof step.exit_code === 'number' ? <span>exit {step.exit_code}</span> : null}
      </div>
      {step.command ? (
        <p className="break-words font-mono text-amber-950 dark:text-amber-100">
          {step.command}
        </p>
      ) : null}
      {step.stderr_tail ? (
        <pre className="max-h-36 overflow-auto whitespace-pre-wrap rounded bg-red-500/10 p-2 font-mono text-[11px] text-red-800 dark:text-red-200">
          {step.stderr_tail}
        </pre>
      ) : null}
      {step.output_tail ? (
        <pre className="max-h-36 overflow-auto whitespace-pre-wrap rounded bg-amber-100/80 p-2 font-mono text-[11px] text-amber-950 dark:bg-amber-950/50 dark:text-amber-100">
          {step.output_tail}
        </pre>
      ) : null}
    </div>
  )
}

export function WorkflowExceptionPanel({
  task,
  actions,
  recoverPending,
  terminal,
  cancelPending,
  onRequestAction,
  onCancelTask,
}: {
  task: Task
  actions: WorkflowExceptionAction[]
  recoverPending: boolean
  terminal: boolean
  cancelPending: boolean
  onRequestAction: (action: WorkflowExceptionAction) => void
  onCancelTask: () => void
}) {
  const exception = task.workflow_exception
  if (!exception) return null

  const title = workflowLabelFromKind(exception.type)
  const step = exception.failing_step
  const details = [
    exception.state ? `state ${exception.state}` : null,
    exception.role ? `role ${exception.role}` : null,
    exception.target_state ? `target ${exception.target_state}` : null,
    exception.target_role ? `target role ${exception.target_role}` : null,
  ].filter(Boolean)

  const retryActions = actions.filter((a) => EXCEPTION_RETRY_FAMILY.includes(a.kind))
  const primaryRetry =
    EXCEPTION_RETRY_PRIMARY_ORDER.map((k) =>
      retryActions.find((a) => a.kind === k && a.enabled),
    ).find(Boolean) ??
    EXCEPTION_RETRY_PRIMARY_ORDER.map((k) => retryActions.find((a) => a.kind === k)).find(
      Boolean,
    ) ??
    retryActions[0] ??
    null
  const secondaryRetries = retryActions.filter((a) => a !== primaryRetry)
  const openInteractive = actions.find((a) => a.kind === 'open_interactive') ?? null
  const standaloneActions = actions.filter(
    (a) => !EXCEPTION_RETRY_FAMILY.includes(a.kind) && a.kind !== 'open_interactive',
  )

  return (
    <section className="rounded-lg border border-amber-300 bg-amber-50 p-4 text-amber-950 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100">
      <div className="space-y-3">
        <div className="space-y-1">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-sm font-semibold">{title}</p>
            <div className="flex flex-wrap gap-2 text-xs">
              {exception.review_id ? (
                <Link
                  to="/tasks/$taskId/$tab"
                  params={{ taskId: task.id, tab: 'review' }}
                  className="font-mono text-primary hover:underline"
                >
                  Review {exception.review_id.slice(0, 8)}
                </Link>
              ) : null}
              {exception.execution_id ? (
                <Link
                  to="/tasks/$taskId/executions/$executionId"
                  params={{ taskId: task.id, executionId: exception.execution_id }}
                  className="font-mono text-primary hover:underline"
                >
                  Execution {exception.execution_id.slice(0, 8)}
                </Link>
              ) : null}
            </div>
          </div>
          <p className="text-sm">{exception.message}</p>
          {details.length > 0 ? (
            <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs opacity-80">
              {details.map((item) => (
                <span key={item}>{item}</span>
              ))}
            </div>
          ) : null}
        </div>

        {step ? <FailingStepDetails exception={exception} /> : null}

        {exception.related_evidence.length > 0 ? (
          <div className="space-y-1 text-xs">
            <p className="font-medium uppercase tracking-wide opacity-75">Related evidence</p>
            <div className="space-y-1">
              {exception.related_evidence.map((item, index) => (
                <p key={`${item.kind}:${item.id ?? index}`} className="break-words opacity-85">
                  <span className="font-medium">{workflowLabelFromKind(item.kind)}</span>
                  {item.id ? <span className="font-mono"> {item.id.slice(0, 8)}</span> : null}
                  {item.message ? <span> - {item.message}</span> : null}
                </p>
              ))}
            </div>
          </div>
        ) : null}

        {(actions.length > 0 || !terminal) ? (
          <div className="flex flex-wrap items-center gap-2 pt-1">
            {primaryRetry ? (
              <div className="flex items-center">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={!primaryRetry.enabled || recoverPending}
                  title={
                    primaryRetry.enabled
                      ? primaryRetry.propagates
                        ? 'This action may automatically advance the task'
                        : undefined
                      : (primaryRetry.disabled_reason ?? undefined)
                  }
                  className={secondaryRetries.length > 0 ? 'rounded-r-none border-r-0' : ''}
                  onClick={() => onRequestAction(primaryRetry)}
                >
                  {primaryRetry.label}
                </Button>
                {secondaryRetries.length > 0 ? (
                  <DropdownMenu>
                    <DropdownMenuTrigger
                      disabled={recoverPending}
                      className="inline-flex h-7 w-6 cursor-pointer items-center justify-center rounded-l-none rounded-r-md border border-input bg-card text-secondary-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
                    >
                      <CaretDown className="h-3 w-3" weight="bold" />
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      {secondaryRetries.map((action, index) => (
                        <DropdownMenuItem
                          key={actionKey(action, index)}
                          disabled={!action.enabled || recoverPending}
                          onClick={() => onRequestAction(action)}
                        >
                          {action.label}
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
                disabled={!openInteractive.enabled || recoverPending}
                onClick={() => onRequestAction(openInteractive)}
              >
                Open Interactive
              </Button>
            ) : null}
            {standaloneActions.map((action, index) => {
              const btn = (
                <Button
                  key={actionKey(action, index)}
                  size="sm"
                  variant="outline"
                  disabled={!action.enabled || recoverPending}
                  onClick={() => onRequestAction(action)}
                >
                  {action.label}
                </Button>
              )
              return action.enabled ? (
                btn
              ) : action.disabled_reason ? (
                <Tooltip key={actionKey(action, index)} content={action.disabled_reason}>
                  <span>{btn}</span>
                </Tooltip>
              ) : (
                btn
              )
            })}
            {!terminal ? (
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
