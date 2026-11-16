import { useState } from 'react'
import { toast } from 'sonner'
import { useDecidePrExternalAction, useTaskPrActionsQuery } from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { getApiErrorCode, getApiErrorMessage, isApiStatus } from '@/lib/api-error'
import { useAuthStore } from '@/stores/auth'
import type { ProjectMemberResponse, Task } from '@/types/generated'

function readable(value: string): string {
  return value.replaceAll('_', ' ')
}

export function TaskPrActionsPanel({
  task,
  members,
}: {
  task: Task
  members: ProjectMemberResponse[]
}) {
  const user = useAuthStore((state) => state.user)
  const query = useTaskPrActionsQuery(task.id)
  const decide = useDecidePrExternalAction()
  const [message, setMessage] = useState<string | null>(null)
  const [pendingActionId, setPendingActionId] = useState<string | null>(null)
  const canDecide = members.some(
    (member) => member.user_id === user?.id && ['owner', 'admin'].includes(member.role),
  ) || Boolean(user?.is_admin)
  const items = query.data?.pages.flatMap((page) => page.items) ?? []

  if (!query.isLoading && !query.isError && items.length === 0) return null

  const decideAction = async (actionId: string, decision: 'approve' | 'reject', version: number) => {
    setMessage(null)
    setPendingActionId(actionId)
    try {
      await decide.mutateAsync({
        taskId: task.id,
        actionId,
        decision,
        body: {
          expected_version: version,
          note: null,
          idempotency_key: crypto.randomUUID(),
        },
      })
      toast.success(decision === 'approve' ? 'PR action approved' : 'PR action rejected')
    } catch (error) {
      if (
        isApiStatus(error, 409) &&
        getApiErrorCode(error) === 'pr_action.version_conflict'
      ) {
        setMessage('This PR action changed while you were reading it. The latest version has been reloaded.')
        await query.refetch()
      } else {
        const text = getApiErrorMessage(error, 'PR action decision could not be recorded')
        setMessage(text)
        toast.error(text)
      }
    } finally {
      setPendingActionId(null)
    }
  }

  return (
    <section className="min-w-0 space-y-3 rounded-lg border p-4" aria-labelledby="pr-actions-title">
      <div>
        <h3 id="pr-actions-title" className="font-semibold">PR actions</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Approve or reject proposed provider actions. Merging is always a human operation outside Forge.
        </p>
      </div>
      {message ? <p role="status" className="text-sm text-muted-foreground">{message}</p> : null}
      {query.isLoading ? <p className="text-sm text-muted-foreground">Loading PR actions…</p> : null}
      {query.isError ? (
        <div role="alert" className="rounded-md border border-destructive/40 p-3 text-sm text-destructive">
          PR actions could not be loaded.{' '}
          <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      <ul className="space-y-2">
        {items.map((action) => {
          const canReview = action.state === 'proposed' && action.authorization_mode === 'human_approval'
          const pending = pendingActionId === action.id
          return (
            <li key={action.id} className="space-y-2 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{readable(action.kind)}</Badge>
                <Badge variant={action.state === 'proposed' ? 'secondary' : 'outline'}>
                  {readable(action.state)}
                </Badge>
                <span className="text-xs text-muted-foreground">
                  Authorization: {readable(action.authorization_mode)} · version {action.version}
                </span>
              </div>
              {action.expected_remote_head_sha ? (
                <p className="break-all text-xs text-muted-foreground">
                  Expected remote head: {action.expected_remote_head_sha}
                </p>
              ) : null}
              {action.body_digest ? (
                <p className="break-all text-xs text-muted-foreground">Body digest: {action.body_digest}</p>
              ) : null}
              {action.proposal_id ? (
                <p className="break-all text-xs text-muted-foreground">Proposal: {action.proposal_id}</p>
              ) : null}
              {canReview ? (
                canDecide ? (
                  <div className="flex flex-wrap gap-2">
                    <Button
                      size="sm"
                      disabled={decide.isPending || pending}
                      onClick={() => void decideAction(action.id, 'approve', action.version)}
                    >
                      {pending && decide.variables?.decision === 'approve' ? 'Approving…' : 'Approve action'}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={decide.isPending || pending}
                      onClick={() => void decideAction(action.id, 'reject', action.version)}
                    >
                      {pending && decide.variables?.decision === 'reject' ? 'Rejecting…' : 'Reject action'}
                    </Button>
                  </div>
                ) : (
                  <p className="text-xs text-muted-foreground">
                    Only a Project owner or admin can decide this action.
                  </p>
                )
              ) : null}
            </li>
          )
        })}
      </ul>
      {query.hasNextPage ? (
        <div className="flex justify-center">
          <Button
            size="sm"
            variant="outline"
            disabled={query.isFetchingNextPage}
            onClick={() => void query.fetchNextPage()}
          >
            {query.isFetchingNextPage ? 'Loading…' : 'Load older actions'}
          </Button>
        </div>
      ) : null}
    </section>
  )
}
