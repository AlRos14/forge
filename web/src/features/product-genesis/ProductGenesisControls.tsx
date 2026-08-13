import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { ArrowRight, CircleNotch, Flask, X } from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import {
  useAgentChatMessagesQuery,
  useAgentChatTurnsQuery,
  useCreateAgentHandoffMutation,
} from '@/features/agent-chat/hooks'
import {
  useCancelProductGenesisMutation,
  useProductGenesisActiveQuery,
  useReadyProductGenesisMutation,
  useStartProductGenesisMutation,
} from './hooks'
import type { ProductMaturity } from './types'
import { productGenesisVersion } from './types'

const maturities: Array<{ value: ProductMaturity; label: string; hint: string }> = [
  { value: 'prototype', label: 'Prototype', hint: 'Cheap learning loop and reversible decisions' },
  { value: 'mvp', label: 'MVP', hint: 'Smallest reliable end-to-end outcome' },
  { value: 'production', label: 'Production', hint: 'Operations, security, support, and release' },
  {
    value: 'critical',
    label: 'Critical',
    hint: 'Safety, recovery, auditability, and rollout gates',
  },
]

function lifecycleLabel(value: string): string {
  return value.replaceAll('_', ' ')
}

export function ProductGenesisControls() {
  const activeQuery = useProductGenesisActiveQuery()
  const startMutation = useStartProductGenesisMutation()
  const cancelMutation = useCancelProductGenesisMutation()
  const readyMutation = useReadyProductGenesisMutation()
  const [open, setOpen] = useState(false)
  const [maturity, setMaturity] = useState<ProductMaturity>('mvp')
  const [initialIdea, setInitialIdea] = useState('')
  const [error, setError] = useState<string | null>(null)
  const active = activeQuery.data?.session ?? null
  const mainMessagesQuery = useAgentChatMessagesQuery(active?.main_chat_id)
  const mainTurnsQuery = useAgentChatTurnsQuery(active?.main_chat_id)
  const handoffMutation = useCreateAgentHandoffMutation(active?.project_id ?? undefined)
  const latestAgentMessage = [...(mainMessagesQuery.data?.items ?? [])]
    .reverse()
    .find((message) => message.author_type === 'agent' && message.status === 'complete')
  const sourceTurn = mainTurnsQuery.data?.find(
    (turn) => turn.response_message_id === latestAgentMessage?.id && turn.status === 'succeeded',
  )

  async function start() {
    setError(null)
    try {
      await startMutation.mutateAsync({
        maturity,
        initial_idea: initialIdea.trim() || null,
        preferred_project_agent_identity_id: null,
      })
      setInitialIdea('')
      setOpen(false)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Product Genesis could not start.')
    }
  }

  async function cancel() {
    if (!active) return
    setError(null)
    try {
      await cancelMutation.mutateAsync({
        sessionId: active.id,
        input: {
          expected_version: productGenesisVersion(active),
          reason: 'cancelled_from_main_chat',
        },
      })
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Product Genesis could not be cancelled.')
    }
  }

  async function ready() {
    if (!active) return
    setError(null)
    try {
      await readyMutation.mutateAsync({
        sessionId: active.id,
        input: { expected_version: productGenesisVersion(active) },
      })
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Product Genesis could not become ready.')
    }
  }

  async function handoff() {
    if (!active?.project_id || !latestAgentMessage) return
    setError(null)
    try {
      await handoffMutation.mutateAsync({
        source_message_id: latestAgentMessage.id,
        source_turn_job_id: sourceTurn?.id ?? null,
        content: latestAgentMessage.content,
        dedupe_key: `product-genesis:${active.id}:handoff`,
      })
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Product Genesis handoff failed.')
    }
  }

  if (activeQuery.isError) {
    return (
      <p className="max-w-xs text-xs text-destructive" role="alert">
        Product Genesis status is unavailable.
      </p>
    )
  }

  return (
    <div className="flex flex-wrap items-center justify-end gap-2">
      {active ? (
        <div className="flex flex-wrap items-center gap-2 rounded-lg border border-ember-border bg-ember-surface px-3 py-2">
          <Flask size={15} className="text-primary" aria-hidden />
          <span className="text-xs text-foreground">
            Genesis · {lifecycleLabel(active.lifecycle)} · {active.maturity}
          </span>
          {active.project_id && active.lifecycle === 'handed_off' ? (
            <Link
              to="/projects/$projectId/chat"
              params={{ projectId: active.project_id }}
              className="inline-flex items-center gap-1 text-xs font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              Continue with Project Agent
              <ArrowRight size={13} aria-hidden />
            </Link>
          ) : null}
          {active.project_id && active.lifecycle === 'ready_for_project' ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void handoff()}
              disabled={!latestAgentMessage || handoffMutation.isPending}
              title={
                latestAgentMessage
                  ? 'Publish the latest completed Main Agent brief'
                  : 'Wait for a completed Main Agent brief before handoff'
              }
            >
              {handoffMutation.isPending ? (
                <CircleNotch size={14} className="animate-spin" />
              ) : (
                <ArrowRight size={14} />
              )}
              Hand off to Project Agent
            </Button>
          ) : null}
          {active.lifecycle === 'discovering' ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void ready()}
              disabled={readyMutation.isPending}
            >
              {readyMutation.isPending ? (
                <CircleNotch size={14} className="animate-spin" />
              ) : (
                <ArrowRight size={14} />
              )}
              Ready for Project
            </Button>
          ) : null}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => void cancel()}
            disabled={cancelMutation.isPending}
            aria-label="Cancel Product Genesis"
          >
            {cancelMutation.isPending ? (
              <CircleNotch size={14} className="animate-spin" />
            ) : (
              <X size={14} />
            )}
            Cancel
          </Button>
        </div>
      ) : (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setOpen((value) => !value)}
        >
          <Flask size={15} aria-hidden />
          Product Genesis
        </Button>
      )}
      {open && !active ? (
        <div className="basis-full rounded-lg border border-border-subtle bg-card p-3 shadow-xs">
          <div className="grid gap-3 sm:grid-cols-[minmax(0,12rem)_minmax(0,1fr)_auto] sm:items-end">
            <label className="grid gap-1 text-xs font-medium text-foreground">
              Maturity
              <select
                value={maturity}
                onChange={(event) => setMaturity(event.target.value as ProductMaturity)}
                className="h-9 rounded-md border border-border bg-background px-2 text-sm font-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                aria-label="Product maturity"
              >
                {maturities.map((item) => (
                  <option key={item.value} value={item.value}>
                    {item.label}
                  </option>
                ))}
              </select>
              <span className="font-normal text-muted-foreground">
                {maturities.find((item) => item.value === maturity)?.hint}
              </span>
            </label>
            <label className="grid gap-1 text-xs font-medium text-foreground">
              Initial idea <span className="font-normal text-muted-foreground">optional</span>
              <textarea
                value={initialIdea}
                onChange={(event) => setInitialIdea(event.target.value)}
                rows={2}
                maxLength={2000}
                placeholder="What should the Main Agent help discover?"
                className="resize-y rounded-md border border-border bg-background px-2 py-2 text-sm font-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                aria-label="Product Genesis initial idea"
              />
            </label>
            <Button type="button" onClick={() => void start()} disabled={startMutation.isPending}>
              {startMutation.isPending ? (
                <CircleNotch size={15} className="animate-spin" />
              ) : (
                <ArrowRight size={15} />
              )}
              Start discovery
            </Button>
          </div>
          {error ? (
            <p className="mt-2 text-xs text-destructive" role="alert">
              {error}
            </p>
          ) : null}
          <p className="mt-2 text-micro text-muted-foreground">
            Uses this existing Main Agent timeline; it does not create another chat.
          </p>
        </div>
      ) : null}
      {error && !open && !active ? (
        <p className="basis-full text-xs text-destructive" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  )
}
