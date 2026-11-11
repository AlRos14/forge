import { useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { qk } from '@/api/query-keys'
import { apiFetch } from '@/api/client'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { getApiErrorMessage } from '@/lib/api-error'
import type { TaskAutonomyLimits, UpdateTaskAutonomyLimitsRequest } from '@/types/generated'

const fields = [
  ['max_turns', 'Coordinator turns'],
  ['max_rework_rounds', 'Correction rounds'],
  ['max_quality_requests', 'Quality requests in the Task'],
  ['max_route_requests', 'Quality requests per chain'],
] as const

export function TaskAutonomyLimitsPanel({
  taskId,
  canManage,
}: {
  taskId: string
  canManage: boolean
}) {
  const client = useQueryClient()
  const path = `/tasks/${taskId}/coordination/limits`
  const query = useQuery({
    queryKey: ['task-autonomy-limits', taskId],
    queryFn: () => apiFetch<TaskAutonomyLimits>(path),
  })
  const [limits, setLimits] = useState<TaskAutonomyLimits | null>(null)
  const [reason, setReason] = useState('')
  useEffect(() => {
    if (query.data) setLimits(query.data)
  }, [query.data])
  const update = useMutation({
    mutationFn: (next: TaskAutonomyLimits) => {
      const request: UpdateTaskAutonomyLimitsRequest = {
        expected_version: query.data!.version,
        limits: next,
        reason: reason.trim() || 'Human authorized additional autonomy rounds',
        idempotency_key: crypto.randomUUID(),
      }
      return apiFetch<TaskAutonomyLimits>(path, { method: 'PUT', body: JSON.stringify(request) })
    },
    onSuccess: async (result) => {
      client.setQueryData(['task-autonomy-limits', taskId], result)
      await Promise.all([
        client.invalidateQueries({ queryKey: qk.taskCoordination(taskId) }),
        client.invalidateQueries({ queryKey: qk.taskQualityRequests(taskId) }),
      ])
      toast.success('Autonomy limits saved. Consumed rounds are preserved.')
    },
    onError: async (error) => {
      await client.invalidateQueries({ queryKey: ['task-autonomy-limits', taskId] })
      toast.error(getApiErrorMessage(error, 'Autonomy limits could not be saved'))
    },
  })
  if (query.isLoading)
    return <p className="text-sm text-muted-foreground">Loading autonomy limits…</p>
  if (!limits || query.isError)
    return (
      <p role="alert" className="text-sm text-destructive">
        Autonomy limits are unavailable.
      </p>
    )
  return (
    <details className="rounded-lg border p-4">
      <summary className="cursor-pointer font-medium">Autonomy limits and extra rounds</summary>
      <p className="my-3 text-sm text-muted-foreground">
        A Project owner or administrator can adjust these separate limits. Increasing quality or
        route limits can re-admit requests blocked only by those budgets; increasing rework rounds
        can resume matching blocked rework intents. Increasing coordinator turns does not change
        those other limits. When the exact blocker is preparation-turn exhaustion, a higher limit
        lets the scheduler retry this Task in its running Project batch on the next tick. The
        scheduler rechecks batch state, Task team, authority, and prerequisites; paused, revoked, or
        completed batches and other blockers remain. This preserves the work history and does not
        add permissions.
      </p>
      <div className="grid gap-3 sm:grid-cols-2">
        {fields.map(([key, label]) => (
          <label key={key} className="space-y-1 text-sm">
            <span>{label}</span>
            <Input
              disabled={!canManage}
              type="number"
              min={1}
              max={1000}
              value={limits[key]}
              onChange={(event) => setLimits({ ...limits, [key]: Number(event.target.value) })}
            />
          </label>
        ))}
      </div>
      <label className="mt-3 block space-y-1 text-sm">
        <span>Reason</span>
        <Input
          disabled={!canManage}
          value={reason}
          onChange={(event) => setReason(event.target.value)}
          placeholder="Why should Forge continue?"
        />
      </label>
      <div className="mt-3 flex flex-wrap gap-2">
        <Button
          type="button"
          disabled={
            !canManage ||
            update.isPending ||
            fields.some(
              ([key]) => !Number.isInteger(limits[key]) || limits[key] < 1 || limits[key] > 1000,
            )
          }
          onClick={() => update.mutate(limits)}
        >
          Save limits
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={!canManage || update.isPending || limits.max_turns > 992}
          onClick={() => update.mutate({ ...query.data!, max_turns: query.data!.max_turns + 8 })}
        >
          Authorize 8 extra coordinator turns
        </Button>
      </div>
    </details>
  )
}
