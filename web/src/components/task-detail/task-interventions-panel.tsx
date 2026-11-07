import { useState, type FormEvent } from 'react'
import { toast } from 'sonner'
import { useCreateTaskIntervention, useTaskInterventionsQuery } from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CollapsibleSection } from '@/components/ui/collapsible-section'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import type {
  Task,
  TaskCoordinationOverviewResponse,
  TaskInterventionKind,
  TaskInterventionState,
} from '@/types/generated'

const interventionLabels: Record<TaskInterventionKind, string> = {
  instruction: 'Instruction',
  decision: 'Plan decision',
  pause: 'Pause',
  resume: 'Resume',
  cancel: 'Cancellation',
  revoke_authorization: 'Authorization revocation',
  request_replan: 'Plan revision request',
}

const stateLabels: Record<TaskInterventionState, string> = {
  recorded: 'Recorded',
  applied: 'Applied',
  requires_human: 'Needs a human decision',
  rejected: 'Rejected',
}

function newIdempotencyKey() {
  return crypto.randomUUID()
}

export function TaskInterventionsPanel({
  task,
  coordination,
}: {
  task: Task
  coordination: TaskCoordinationOverviewResponse
}) {
  const query = useTaskInterventionsQuery(task.id)
  const createIntervention = useCreateTaskIntervention()
  const [instruction, setInstruction] = useState('')
  const [objective, setObjective] = useState(task.title)
  const [constraints, setConstraints] = useState('')
  const [acceptanceCriteria, setAcceptanceCriteria] = useState('')
  const [context, setContext] = useState('')
  const [actionError, setActionError] = useState<string | null>(null)

  async function recordInstruction(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const content = instruction.trim()
    if (!content) return
    setActionError(null)
    try {
      const response = await createIntervention.mutateAsync({
        taskId: task.id,
        body: {
          kind: 'instruction',
          expected_task_version: task.version,
          expected_coordination_version: null,
          expected_lifecycle_version: null,
          instruction: content,
          plan_artifact_id: null,
          decision: null,
          rationale: null,
          reason: null,
          brief: null,
          idempotency_key: newIdempotencyKey(),
        },
      })
      if (response.state === 'rejected') {
        throw new Error(
          response.failure_message ?? response.failure_code ?? 'Instruction was rejected',
        )
      }
      setInstruction('')
      toast.success('Instruction recorded in the Task intervention inbox')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Instruction could not be recorded')
      setActionError(message)
      toast.error(message)
    }
  }

  async function requestReplan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!coordination.can_request || !objective.trim()) return
    setActionError(null)
    try {
      const response = await createIntervention.mutateAsync({
        taskId: task.id,
        body: {
          kind: 'request_replan',
          expected_task_version: coordination.task_version,
          expected_coordination_version: coordination.authorization_version,
          expected_lifecycle_version: null,
          instruction: null,
          plan_artifact_id: null,
          decision: null,
          rationale: null,
          reason: null,
          brief: {
            expected_task_version: coordination.task_version,
            expected_coordination_version: coordination.authorization_version,
            objective: objective.trim(),
            constraints: constraints.trim() || null,
            acceptance_criteria: acceptanceCriteria
              .split('\n')
              .map((criterion) => criterion.trim())
              .filter(Boolean),
            context: context.trim() || null,
            idempotency_key: newIdempotencyKey(),
          },
          idempotency_key: newIdempotencyKey(),
        },
      })
      if (response.state === 'rejected') {
        throw new Error(
          response.failure_message ?? response.failure_code ?? 'Plan revision was rejected',
        )
      }
      toast.success('Plan revision request recorded')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Plan revision request could not be recorded')
      setActionError(message)
      toast.error(message)
    }
  }

  return (
    <section
      className="min-w-0 space-y-4 rounded-lg border p-4"
      aria-labelledby="task-interventions-title"
    >
      <div className="min-w-0">
        <h3 id="task-interventions-title" className="font-semibold">
          Task intervention inbox
        </h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Human actions and Project Copilot instructions appear in this durable Task record. An
          instruction is recorded as a message; it does not authorize work or change the plan by
          itself.
        </p>
      </div>

      {query.isLoading ? (
        <p className="text-sm text-muted-foreground">Loading interventions…</p>
      ) : null}
      {query.isError ? (
        <div
          role="alert"
          className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          The intervention inbox could not be loaded.{' '}
          <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      {query.data?.items.length ? (
        <ol className="space-y-2">
          {query.data.items.map((item) => (
            <li key={item.id} className="min-w-0 rounded-md border p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <span className="break-words text-sm font-medium">
                    {interventionLabels[item.kind]}
                  </span>
                  <Badge
                    variant={
                      item.state === 'rejected'
                        ? 'destructive'
                        : item.state === 'requires_human'
                          ? 'secondary'
                          : 'outline'
                    }
                  >
                    {stateLabels[item.state]}
                  </Badge>
                </div>
                <span className="text-xs text-muted-foreground">
                  {item.source_kind === 'project_copilot'
                    ? 'Project Copilot'
                    : item.source_kind === 'human'
                      ? 'Human'
                      : item.source_kind.replaceAll('_', ' ')}
                  {' · '}
                  {new Date(item.created_at).toLocaleString()}
                </span>
              </div>
              {item.failure_message ? (
                <p role="alert" className="mt-2 break-words text-sm text-destructive">
                  {item.failure_message}
                </p>
              ) : null}
              {item.failure_code ? (
                <p className="mt-1 text-xs text-muted-foreground">Failure: {item.failure_code}</p>
              ) : null}
              <details className="mt-2 text-xs text-muted-foreground">
                <summary className="w-fit cursor-pointer">Intervention references</summary>
                <div className="mt-1 space-y-1">
                  <div className="flex min-w-0 items-center gap-1">
                    <code className="min-w-0 break-all">{item.id}</code>
                    <CopyReferenceButton value={item.id} label="Intervention ID" />
                  </div>
                  {item.proposal_id ? (
                    <p>
                      Proposal: <code className="break-all">{item.proposal_id}</code>
                    </p>
                  ) : null}
                  {item.decision_id ? (
                    <p>
                      Decision: <code className="break-all">{item.decision_id}</code>
                    </p>
                  ) : null}
                  {item.coordination_request_id ? (
                    <p>
                      Coordination request:{' '}
                      <code className="break-all">{item.coordination_request_id}</code>
                    </p>
                  ) : null}
                  <p>
                    Source reference: <code className="break-all">{item.source_id}</code>
                  </p>
                  <p>Version {item.version}</p>
                </div>
              </details>
            </li>
          ))}
        </ol>
      ) : query.data ? (
        <p className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
          No interventions have been recorded for this Task.
        </p>
      ) : null}

      {actionError ? (
        <p role="alert" className="text-sm text-destructive">
          {actionError}
        </p>
      ) : null}
      <CollapsibleSection title="Record an instruction" contentClassName="pt-3">
        <form
          onSubmit={(event) => void recordInstruction(event)}
          className="space-y-3 rounded-md bg-muted/20 p-3"
        >
          <div className="space-y-2">
            <Label htmlFor="task-instruction">Instruction for the Task coordinator</Label>
            <Textarea
              id="task-instruction"
              value={instruction}
              onChange={(event) => setInstruction(event.target.value)}
              maxLength={16_384}
              placeholder="Clarify future work or a constraint…"
              required
            />
          </div>
          <Button
            size="sm"
            type="submit"
            disabled={!instruction.trim() || createIntervention.isPending}
          >
            {createIntervention.isPending ? 'Recording…' : 'Record instruction'}
          </Button>
        </form>
      </CollapsibleSection>

      {coordination.can_request ? (
        <CollapsibleSection title="Ask for a plan revision" contentClassName="pt-3">
          <form
            onSubmit={(event) => void requestReplan(event)}
            className="space-y-3 rounded-md bg-muted/20 p-3"
          >
            <p className="text-sm text-muted-foreground">
              This creates a versioned coordination request against the current authorization. It
              does not edit the existing plan in place.
            </p>
            <div className="space-y-2">
              <Label htmlFor="replan-objective">Updated outcome</Label>
              <Textarea
                id="replan-objective"
                value={objective}
                onChange={(event) => setObjective(event.target.value)}
                required
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="replan-constraints">Constraints</Label>
              <Textarea
                id="replan-constraints"
                value={constraints}
                onChange={(event) => setConstraints(event.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="replan-criteria">Acceptance criteria</Label>
              <Textarea
                id="replan-criteria"
                value={acceptanceCriteria}
                onChange={(event) => setAcceptanceCriteria(event.target.value)}
                placeholder="One criterion per line"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="replan-context">Context</Label>
              <Textarea
                id="replan-context"
                value={context}
                onChange={(event) => setContext(event.target.value)}
              />
            </div>
            <Button
              size="sm"
              type="submit"
              disabled={!objective.trim() || createIntervention.isPending}
            >
              {createIntervention.isPending ? 'Recording…' : 'Request plan revision'}
            </Button>
          </form>
        </CollapsibleSection>
      ) : null}
    </section>
  )
}
