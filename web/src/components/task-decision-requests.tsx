import { useState } from 'react'
import { useAnswerTaskDecision, useTaskDecisionsQuery } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

export function TaskDecisionRequests({ taskId }: { taskId: string }) {
  const query = useTaskDecisionsQuery(taskId)
  const answer = useAnswerTaskDecision(taskId)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const pending = (query.data ?? []).filter((request) => request.status === 'pending')
  if (pending.length === 0) return null
  return (
    <section className="space-y-3 rounded-lg border border-amber-500/30 bg-amber-500/10 p-4" aria-label="Pending decisions">
      <h3 className="text-sm font-semibold">Decision required</h3>
      {pending.map((request) => (
        <div key={request.id} className="space-y-2 rounded-md border border-border-subtle bg-card p-3">
          <p className="font-mono text-micro uppercase text-muted-foreground">{request.authority_scope}</p>
          {request.context ? <p className="text-sm">{request.context}</p> : null}
          {request.questions.map((question, index) => {
            const key = String(question.id ?? index)
            return (
              <label key={key} className="block space-y-1 text-sm">
                <span>{String(question.question ?? 'Answer required')}</span>
                <Input value={drafts[`${request.id}:${key}`] ?? ''} onChange={(event) => setDrafts((current) => ({ ...current, [`${request.id}:${key}`]: event.target.value }))} />
              </label>
            )
          })}
          {request.authority_scope === 'task' ? (
            <Button size="sm" disabled={answer.isPending} onClick={() => void answer.mutateAsync({
              requestId: request.id,
              answers: Object.fromEntries(request.questions.map((question, index) => {
                const key = String(question.id ?? index)
                return [key, drafts[`${request.id}:${key}`] ?? '']
              })),
            })}>Submit answers</Button>
          ) : (
            <p className="text-xs text-muted-foreground">Record this as a Project Decision and reconcile the active baseline.</p>
          )}
        </div>
      ))}
    </section>
  )
}
