import { useState } from 'react'
import { toast } from 'sonner'
import { useAnswerHumanDecision, useTaskHumanDecisionsQuery } from '@/api/hooks'
import { useAuthStore } from '@/stores/auth'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorCode, getApiErrorMessage, isApiStatus } from '@/lib/api-error'
import type { HumanDecisionResponse, ProjectMemberResponse, Task } from '@/types/generated'
import {
  authorityEffectLabels,
  authorityEffectVariant,
  canAnswerDecisionOption,
  decisionStateLabels,
  isDecisionOpen,
} from './task-verification'

export function TaskHumanDecisionCard({
  task,
  decision,
  canAuthority,
}: {
  task: Task
  decision: HumanDecisionResponse
  canAuthority: boolean
}) {
  const answer = useAnswerHumanDecision()
  const [optionId, setOptionId] = useState(decision.recommended_option_id ?? '')
  const [note, setNote] = useState('')
  const [message, setMessage] = useState<string | null>(null)
  const open = isDecisionOpen(decision)
  const selected = decision.options.find((option) => option.id === optionId)
  const canSubmit = open && Boolean(selected) && canAnswerDecisionOption(selected!.authority_effect, canAuthority)

  async function submit() {
    if (!selected || !canSubmit) return
    setMessage(null)
    try {
      await answer.mutateAsync({
        taskId: task.id,
        decisionId: decision.id,
        body: {
          expected_version: decision.version,
          option_id: selected.id,
          note: note.trim() || null,
          idempotency_key: crypto.randomUUID(),
        },
      })
      toast.success('Decision recorded')
      setNote('')
    } catch (error) {
      if (isApiStatus(error, 409)) {
        // The mutation's onSettled refetches the list; tell the person what happened.
        const code = getApiErrorCode(error)
        setMessage(
          code === 'human_decision.not_open'
            ? 'This decision was already answered or closed. The latest state is shown.'
            : 'This decision changed while you were reading it. The latest version is shown; review it and answer again.',
        )
        return
      }
      const text = getApiErrorMessage(error, 'Decision could not be recorded')
      setMessage(text)
      toast.error(text)
    }
  }

  return (
    <article className="min-w-0 space-y-3 rounded-md border p-3" data-testid="human-decision-card">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <p className="min-w-0 break-words text-sm font-medium">{decision.question}</p>
        <Badge variant={open ? 'secondary' : 'outline'}>{decisionStateLabels[decision.state]}</Badge>
      </div>

      <ul className="space-y-2" aria-label="Options">
        {decision.options.map((option) => {
          const recommended = option.id === decision.recommended_option_id
          const chosen = decision.answered_option_id === option.id
          const blocked = !canAnswerDecisionOption(option.authority_effect, canAuthority)
          return (
            <li
              key={option.id}
              className={`rounded-md border p-2 text-sm ${
                recommended ? 'border-primary/50 bg-primary/5' : ''
              }`}
            >
              <label className="flex min-w-0 items-start gap-2">
                {open ? (
                  <input
                    type="radio"
                    name={`decision-${decision.id}`}
                    value={option.id}
                    checked={optionId === option.id}
                    disabled={blocked}
                    onChange={() => setOptionId(option.id)}
                    className="mt-1"
                  />
                ) : null}
                <span className="min-w-0 space-y-1">
                  <span className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">{option.label}</span>
                    {recommended ? <Badge variant="default">Recommended</Badge> : null}
                    {chosen ? <Badge variant="outline">Chosen</Badge> : null}
                    <Badge variant={authorityEffectVariant(option.authority_effect)}>
                      {authorityEffectLabels[option.authority_effect]}
                    </Badge>
                  </span>
                  <span className="block break-words text-muted-foreground">
                    {option.description}
                  </span>
                  <span className="block break-words text-xs text-muted-foreground">
                    Consequence: {option.consequence}
                  </span>
                  {blocked && open ? (
                    <span className="block text-xs text-destructive">
                      Only a Project owner or admin can choose this option.
                    </span>
                  ) : null}
                </span>
              </label>
            </li>
          )
        })}
      </ul>

      {decision.recommendation_rationale ? (
        <p className="break-words text-sm text-muted-foreground">
          <span className="font-medium text-foreground">Why this is recommended:</span>{' '}
          {decision.recommendation_rationale}
        </p>
      ) : null}

      {decision.evidence_links.length ? (
        <div>
          <p className="text-xs font-medium">Evidence</p>
          <ul className="mt-1 space-y-1 text-xs text-muted-foreground">
            {decision.evidence_links.map((link) => (
              <li key={`${link.kind}:${link.id}`} className="flex min-w-0 flex-wrap items-center gap-1">
                <Badge variant="outline">{link.kind.replaceAll('_', ' ')}</Badge>
                <code className="break-all">{link.id}</code>
                <CopyReferenceButton value={link.id} label={`${link.kind} ID`} />
                {link.digest ? <span className="break-all">sha256 {link.digest}</span> : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {message ? (
        <div role="alert" className="rounded-md border border-amber-400/60 bg-amber-50 p-2 text-sm text-amber-950 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100">
          {message}
        </div>
      ) : null}

      {open ? (
        <div className="space-y-2">
          <Textarea
            aria-label="Note (optional)"
            value={note}
            onChange={(event) => setNote(event.target.value)}
            placeholder="Optional note recorded with your answer"
          />
          <Button size="sm" disabled={!canSubmit || answer.isPending} onClick={() => void submit()}>
            {answer.isPending ? 'Recording…' : 'Record answer'}
          </Button>
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          {decision.state === 'answered'
            ? `Answered${decision.answered_at ? ` at ${decision.answered_at}` : ''}${
                decision.answer_note ? `: ${decision.answer_note}` : ''
              }`
            : 'This decision is closed and can no longer be answered.'}
        </p>
      )}
    </article>
  )
}

export function TaskHumanDecisionsPanel({
  task,
  members,
}: {
  task: Task
  members: ProjectMemberResponse[]
}) {
  const user = useAuthStore((state) => state.user)
  const query = useTaskHumanDecisionsQuery(task.id)
  const canAuthority = members.some(
    (member) => member.user_id === user?.id && ['owner', 'admin'].includes(member.role),
  )
  const items = query.data?.items ?? []
  if (!query.isLoading && !query.isError && items.length === 0) return null
  return (
    <section className="min-w-0 space-y-3 rounded-lg border p-4" aria-labelledby="human-decisions-title">
      <h3 id="human-decisions-title" className="font-semibold">
        Decisions for a person
      </h3>
      {query.isLoading ? <p className="text-sm text-muted-foreground">Loading decisions…</p> : null}
      {query.isError ? (
        <div role="alert" className="rounded-md border border-destructive/40 p-3 text-sm text-destructive">
          Decisions could not be loaded.{' '}
          <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      {items.map((decision) => (
        <TaskHumanDecisionCard
          key={`${decision.id}:${decision.version}`}
          task={task}
          decision={decision}
          canAuthority={canAuthority}
        />
      ))}
    </section>
  )
}
