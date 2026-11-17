import { useMemo, useRef, useState } from 'react'
import { ArrowClockwise, GitBranch, Pause, Play, Prohibit } from '@phosphor-icons/react'
import { Link } from '@tanstack/react-router'
import { useTranslation } from 'react-i18next'
import { useMembersQuery, useProjectQuery } from '@/api/hooks'
import { ErrorBanner } from '@/components/error-banner'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { useAuthStore } from '@/stores/auth'
import type {
  ProjectPlanBatchResponse,
  ProjectPlanEdge,
  ProjectPlanNodeResponse,
  ProjectPlanProposalResponse,
  ProjectPlanSource,
} from '@/types/generated'
import { getApiErrorCode, getApiErrorMessage } from '@/lib/api-error'
import {
  NODE_HEIGHT,
  NODE_WIDTH,
  edgePath,
  formatMinuteRange,
  layoutProjectGraph,
} from './graph-layout'
import { useProjectPlanMutations, useProjectPlanQuery } from './hooks'

type PlanNode = ProjectPlanNodeResponse
type EdgeNode = { key: string; task_id: string | null; title: string }
type BatchOperation = 'pause' | 'resume' | 'revoke'
type Translator = ReturnType<typeof useTranslation>['t']
const NON_REPLANNABLE_LIFECYCLES = new Set([
  'active',
  'ready_to_merge',
  'merging',
  'done',
  'cancelled',
])

function isBatchTaskSelectable(node: PlanNode) {
  return !NON_REPLANNABLE_LIFECYCLES.has(node.lifecycle_state)
}

function humanize(value: string) {
  return value.replaceAll('_', ' ').replace(/\b\w/g, (letter) => letter.toUpperCase())
}

function phaseLabel(phase: string | null, noPhase: string) {
  return phase?.trim() || noPhase
}

function validEstimate(node: PlanNode) {
  return (
    node.estimate_min_minutes !== null &&
    node.estimate_max_minutes !== null &&
    Number.isFinite(node.estimate_min_minutes) &&
    Number.isFinite(node.estimate_max_minutes) &&
    node.estimate_min_minutes >= 0 &&
    node.estimate_min_minutes <= node.estimate_max_minutes
  )
}

function estimateIsComplete(nodes: PlanNode[]) {
  return nodes.length > 0 && nodes.every(validEstimate)
}

function shortDigest(value: string) {
  return value.length > 24 ? `${value.slice(0, 12)}…${value.slice(-8)}` : value
}

function sourceLabel(sourceIds: string[], sources: ProjectPlanSource[], missingLabel: string) {
  const sourceById = new Map(sources.map((source) => [source.id, source]))
  return sourceIds.map((id) => sourceById.get(id)?.label ?? `${missingLabel}: ${id}`)
}

function getBatchActions(state: string): BatchOperation[] {
  if (state === 'running') return ['pause', 'revoke']
  if (state === 'paused') return ['resume', 'revoke']
  return []
}

function SourceList({
  sources,
  title,
  t,
}: {
  sources: ProjectPlanSource[]
  title: string
  t: Translator
}) {
  if (sources.length === 0) return null
  return (
    <section aria-label={title} className="space-y-2">
      <h4 className="text-xs font-semibold text-foreground">{title}</h4>
      <ul className="space-y-2">
        {sources.map((source) => (
          <li
            key={source.id}
            className="min-w-0 rounded-md border border-border-subtle bg-background p-3"
          >
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <span className="break-words text-xs font-medium">{source.label}</span>
              <span className="rounded-full border border-border-subtle px-2 py-0.5 font-mono text-micro text-muted-foreground">
                {humanize(source.kind)}
              </span>
              <span className="text-micro text-muted-foreground">
                {source.revision
                  ? t('projectPlan.sourceRevision', { revision: source.revision })
                  : t('projectPlan.sourceRevisionUnknown')}
              </span>
            </div>
            {source.reference ? (
              <p className="mt-1 break-all font-mono text-micro text-muted-foreground">
                {source.reference}
              </p>
            ) : null}
            {source.excerpt ? (
              <p className="mt-2 whitespace-pre-wrap break-words text-xs text-muted-foreground">
                {source.excerpt}
              </p>
            ) : null}
          </li>
        ))}
      </ul>
    </section>
  )
}

function EdgeTable({
  edges,
  nodes,
  sources,
  noDependencies,
  t,
}: {
  edges: ProjectPlanEdge[]
  nodes: EdgeNode[]
  sources: ProjectPlanSource[]
  noDependencies: string
  t: Translator
}) {
  const nodeByKey = new Map(nodes.map((node) => [node.key, node]))
  if (edges.length === 0) {
    return <p className="text-xs text-muted-foreground">{noDependencies}</p>
  }

  return (
    <div className="overflow-x-auto rounded-md border border-border-subtle">
      <table className="w-full min-w-[42rem] border-collapse text-left text-xs">
        <caption className="sr-only">{t('projectPlan.dependencyTable')}</caption>
        <thead className="bg-muted/50 text-muted-foreground">
          <tr>
            <th scope="col" className="px-3 py-2 font-medium">
              {t('projectPlan.predecessor')}
            </th>
            <th scope="col" className="px-3 py-2 font-medium">
              {t('projectPlan.successor')}
            </th>
            <th scope="col" className="px-3 py-2 font-medium">
              {t('projectPlan.edgeKind')}
            </th>
            <th scope="col" className="px-3 py-2 font-medium">
              {t('projectPlan.sources')}
            </th>
          </tr>
        </thead>
        <tbody>
          {edges.map((edge) => {
            const predecessor = nodeByKey.get(edge.predecessor_key)
            const successor = nodeByKey.get(edge.successor_key)
            return (
              <tr
                key={`${edge.predecessor_key}:${edge.successor_key}`}
                className="border-t border-border-subtle"
              >
                <td className="px-3 py-2">
                  {predecessor ? (
                    predecessor.task_id ? (
                      <Link
                        to="/tasks/$taskId"
                        params={{ taskId: predecessor.task_id }}
                        className="text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        {predecessor.title}
                      </Link>
                    ) : (
                      predecessor.title
                    )
                  ) : (
                    edge.predecessor_key
                  )}
                </td>
                <td className="px-3 py-2">
                  {successor ? (
                    successor.task_id ? (
                      <Link
                        to="/tasks/$taskId"
                        params={{ taskId: successor.task_id }}
                        className="text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        {successor.title}
                      </Link>
                    ) : (
                      successor.title
                    )
                  ) : (
                    edge.successor_key
                  )}
                </td>
                <td className="px-3 py-2">
                  <span
                    className={
                      edge.inferred
                        ? 'rounded-full border border-amber-500/40 bg-amber-500/10 px-2 py-0.5 text-amber-900 dark:text-amber-200'
                        : 'rounded-full border border-border-subtle bg-muted px-2 py-0.5 text-muted-foreground'
                    }
                  >
                    {edge.inferred ? t('projectPlan.inferred') : t('projectPlan.explicit')}
                  </span>
                </td>
                <td className="px-3 py-2 text-muted-foreground">
                  {sourceLabel(edge.source_ids, sources, t('projectPlan.unknownSource')).join(
                    ', ',
                  ) || '—'}
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

function ProposalReview({
  proposal,
  graphVersion,
  canApply,
  isApplying,
  canRetryApply,
  onApply,
  t,
}: {
  proposal: ProjectPlanProposalResponse
  graphVersion: number
  canApply: boolean
  isApplying: boolean
  canRetryApply: boolean
  onApply: () => void
  t: Translator
}) {
  const isStale = proposal.base_version !== graphVersion
  const applyAllowed = canApply && proposal.state === 'proposed' && (!isStale || canRetryApply)

  return (
    <details className="rounded-lg border border-border-subtle bg-card">
      <summary className="flex cursor-pointer list-none flex-wrap items-center justify-between gap-3 px-4 py-3 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        <span className="min-w-0">
          <span className="block text-sm font-medium">
            {t('projectPlan.proposalSummary', {
              count: proposal.document.nodes.length,
              state: humanize(proposal.state),
            })}
          </span>
          <span className="mt-1 block break-all font-mono text-micro text-muted-foreground">
            {t('projectPlan.proposalRevision', { version: proposal.base_version })} ·{' '}
            {shortDigest(proposal.digest)}
          </span>
        </span>
        <span aria-hidden="true" className="text-xs text-primary">
          {t('projectPlan.inspectProposal')}
        </span>
      </summary>
      <div className="space-y-4 border-t border-border-subtle p-4">
        {isStale && canRetryApply ? (
          <p
            role="status"
            className="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs"
          >
            {t('projectPlan.incompleteApplyRetryHint')}
          </p>
        ) : isStale ? (
          <p
            role="status"
            className="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs"
          >
            {t('projectPlan.staleProposal')}
          </p>
        ) : null}
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-xs text-muted-foreground">{t('projectPlan.applyExplanation')}</p>
          <Button type="button" size="sm" disabled={!applyAllowed || isApplying} onClick={onApply}>
            {isApplying
              ? t('projectPlan.applying')
              : canRetryApply
                ? t('projectPlan.retryIncompleteApply')
                : t('projectPlan.applyProposal')}
          </Button>
        </div>
        {!canApply && proposal.state === 'proposed' ? (
          <p className="text-xs text-muted-foreground">{t('projectPlan.writerRequiredApply')}</p>
        ) : null}
        <ul className="grid gap-3 xl:grid-cols-2">
          {proposal.document.nodes.map((node) => (
            <li key={node.key} className="min-w-0 rounded-md border border-border-subtle p-3">
              <div className="flex flex-wrap items-start justify-between gap-2">
                <div className="min-w-0">
                  <h4 className="break-words text-sm font-medium">{node.title}</h4>
                  <p className="mt-1 break-all font-mono text-micro text-muted-foreground">
                    {node.task_id
                      ? t('projectPlan.mapExistingTask', { id: node.task_id })
                      : t('projectPlan.createTask')}
                  </p>
                </div>
                <span className="rounded-full border border-border-subtle bg-muted px-2 py-0.5 text-micro text-muted-foreground">
                  {phaseLabel(node.phase, t('projectPlan.noPhase'))}
                </span>
              </div>
              {node.description ? (
                <p className="mt-2 whitespace-pre-wrap break-words text-xs text-muted-foreground">
                  {node.description}
                </p>
              ) : null}
              {node.acceptance_criteria.length > 0 ? (
                <ul className="mt-2 list-disc space-y-1 pl-5 text-xs text-muted-foreground">
                  {node.acceptance_criteria.map((criterion, index) => (
                    <li key={`${node.key}:criterion:${index}`}>{criterion}</li>
                  ))}
                </ul>
              ) : null}
              <p className="mt-2 text-xs text-muted-foreground">
                {node.estimate_min_minutes !== null && node.estimate_max_minutes !== null
                  ? formatMinuteRange(node.estimate_min_minutes, node.estimate_max_minutes)
                  : t('projectPlan.estimateUnknown')}
              </p>
              {node.source_ids.length > 0 ? (
                <p className="mt-2 break-words text-micro text-muted-foreground">
                  {t('projectPlan.sources')}:{' '}
                  {sourceLabel(
                    node.source_ids,
                    proposal.document.sources,
                    t('projectPlan.unknownSource'),
                  ).join(', ')}
                </p>
              ) : null}
            </li>
          ))}
        </ul>
        <EdgeTable
          edges={proposal.document.edges}
          nodes={proposal.document.nodes.map((node) => ({
            key: node.key,
            task_id: node.task_id,
            title: node.title,
          }))}
          sources={proposal.document.sources}
          noDependencies={t('projectPlan.proposalNoDependencies')}
          t={t}
        />
        <SourceList sources={proposal.document.sources} title={t('projectPlan.provenance')} t={t} />
      </div>
    </details>
  )
}

function TaskNodeCard({
  node,
  projectId,
  x,
  y,
  selected,
  canManage,
  canSelect,
  sources,
  t,
  onToggle,
}: {
  node: PlanNode
  projectId: string
  x: number
  y: number
  selected: boolean
  canManage: boolean
  canSelect: boolean
  sources: ProjectPlanSource[]
  t: Translator
  onToggle: (taskId: string) => void
}) {
  return (
    <article
      aria-labelledby={`project-plan-node-${node.task_id}`}
      className="absolute flex flex-col overflow-y-auto rounded-lg border border-border-subtle bg-card p-3 shadow-sm"
      style={{ left: x, top: y, width: NODE_WIDTH, height: NODE_HEIGHT }}
    >
      <div className="flex min-w-0 items-start gap-2">
        <input
          type="checkbox"
          className="mt-1 size-4 shrink-0 accent-primary"
          aria-label={t('projectPlan.selectTask', { title: node.title })}
          checked={selected}
          disabled={!canManage || !canSelect}
          onChange={() => onToggle(node.task_id)}
        />
        <div className="min-w-0 flex-1">
          <h3
            id={`project-plan-node-${node.task_id}`}
            className="line-clamp-2 break-words text-sm font-semibold"
          >
            <Link
              to="/tasks/$taskId"
              params={{ taskId: node.task_id }}
              className="hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {node.title}
            </Link>
          </h3>
          <p className="mt-1 break-all font-mono text-micro text-muted-foreground">
            {node.task_id}
          </p>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-1.5 text-micro">
        <span className="rounded-full border border-border-subtle bg-muted px-2 py-0.5">
          {t('projectPlan.taskLifecycle')}: {humanize(node.lifecycle_state)}
        </span>
        <span className="rounded-full border border-border-subtle bg-muted px-2 py-0.5">
          {t('projectPlan.dispatchState')}: {humanize(node.dispatch_state)}
        </span>
        <span className="rounded-full border border-border-subtle px-2 py-0.5 text-muted-foreground">
          {t('projectPlan.readiness')}: {humanize(node.readiness)}
        </span>
        <span className="rounded-full border border-border-subtle px-2 py-0.5 text-muted-foreground">
          {phaseLabel(node.phase, t('projectPlan.noPhase'))}
        </span>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {node.estimate_min_minutes !== null && node.estimate_max_minutes !== null
          ? formatMinuteRange(node.estimate_min_minutes, node.estimate_max_minutes)
          : t('projectPlan.estimateUnknown')}
        {' · '}
        {t('projectPlan.runningExecutions', { count: node.running_executions })}
      </p>
      {!canSelect ? (
        <p className="mt-2 text-xs text-muted-foreground">{t('projectPlan.batchNotSelectable')}</p>
      ) : null}
      {node.blockers.length > 0 ? (
        <details className="mt-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-2 py-1.5">
          <summary className="cursor-pointer text-xs font-medium text-amber-900 dark:text-amber-200">
            {t('projectPlan.blockersCount', { count: node.blockers.length })}
          </summary>
          <ul className="mt-1 space-y-1 break-words font-mono text-micro text-muted-foreground">
            {node.blockers.map((blocker) => (
              <li key={blocker}>{blocker}</li>
            ))}
          </ul>
          {node.blockers.some((blocker) => blocker.startsWith('coordination:')) ? (
            <Link
              to="/projects/$projectId/settings/$tab"
              params={{ projectId, tab: 'workers' }}
              className="mt-2 inline-block text-xs text-primary hover:underline"
            >
              {t('projectPlan.configureCoordination')}
            </Link>
          ) : null}
        </details>
      ) : null}
      {node.unsatisfied_task_ids.length > 0 ? (
        <div className="mt-2 border-t border-border-subtle pt-2">
          <p className="text-micro font-medium text-muted-foreground">
            {t('projectPlan.waitingFor')}
          </p>
          <ul className="mt-1 flex flex-wrap gap-x-2 gap-y-1 text-xs">
            {node.unsatisfied_task_ids.map((taskId) => (
              <li key={taskId}>
                <Link
                  to="/tasks/$taskId"
                  params={{ taskId }}
                  className="text-primary hover:underline"
                >
                  {taskId}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {node.source_ids.length > 0 ? (
        <p className="mt-auto pt-2 text-micro text-muted-foreground">
          {t('projectPlan.sources')}:{' '}
          {sourceLabel(node.source_ids, sources, t('projectPlan.unknownSource')).join(', ')}
        </p>
      ) : null}
    </article>
  )
}

export function ProjectPlanPage({ projectId }: { projectId: string }) {
  const { t } = useTranslation()
  const user = useAuthStore((state) => state.user)
  const graphQuery = useProjectPlanQuery(projectId)
  const projectQuery = useProjectQuery(projectId)
  const membersQuery = useMembersQuery(projectId)
  const mutations = useProjectPlanMutations(projectId)
  const [phaseFilter, setPhaseFilter] = useState('__all__')
  const [selectedTaskIds, setSelectedTaskIds] = useState<Set<string>>(() => new Set())
  const [parallelLimit, setParallelLimit] = useState('1')
  const [actionError, setActionError] = useState<unknown>(null)
  const [retryableApplyProposalIds, setRetryableApplyProposalIds] = useState<Set<string>>(
    () => new Set(),
  )
  const requestKeys = useRef(new Map<string, string>())

  const member = membersQuery.data?.find((item) => item.user_id === user?.id)
  const hasCanonicalOwnerMembership = Boolean(
    projectQuery.data?.owner_id &&
    membersQuery.data?.some(
      (item) => item.user_id === projectQuery.data?.owner_id && item.role === 'owner',
    ),
  )
  const isCanonicalOwner = member?.role === 'owner' && projectQuery.data?.owner_id === user?.id
  const canWritePlan = Boolean(
    hasCanonicalOwnerMembership && member && ['owner', 'admin', 'member'].includes(member.role),
  )
  const canManageBatch = Boolean(
    hasCanonicalOwnerMembership && (member?.role === 'admin' || isCanonicalOwner),
  )
  const graph = graphQuery.data
  const nodes = graph?.nodes ?? []
  const phases = [...new Set(nodes.map((node) => node.phase?.trim() || ''))].sort((a, b) =>
    a.localeCompare(b),
  )
  const visibleNodes =
    phaseFilter === '__all__'
      ? nodes
      : nodes.filter((node) => (node.phase?.trim() || '') === phaseFilter)
  const selectableNodes = nodes.filter(isBatchTaskSelectable)
  const visibleSelectableNodes = visibleNodes.filter(isBatchTaskSelectable)
  const visibleNodeIds = new Set(visibleNodes.map((node) => node.key))
  const visibleEdges = (graph?.edges ?? []).filter(
    (edge) => visibleNodeIds.has(edge.predecessor_key) && visibleNodeIds.has(edge.successor_key),
  )
  const layout = useMemo(
    () =>
      layoutProjectGraph(
        visibleNodes.map((node) => ({ key: node.key, parallel_level: node.parallel_level })),
      ),
    [visibleNodes],
  )
  const activeTasks = nodes.filter((node) => node.running_executions > 0)
  const runningExecutionCount = nodes.reduce((sum, node) => sum + node.running_executions, 0)
  const readyCount = nodes.filter((node) => node.readiness === 'ready').length
  const maxLevelWidth = Math.max(
    0,
    ...[...new Set(nodes.map((node) => node.parallel_level))].map(
      (level) => nodes.filter((node) => node.parallel_level === level).length,
    ),
  )
  const forecastsAreValid = Boolean(
    graph &&
    estimateIsComplete(nodes) &&
    !graph.forecast.incomplete_estimates &&
    graph.forecast.min_minutes !== null &&
    graph.forecast.max_minutes !== null &&
    graph.forecast.min_minutes <= graph.forecast.max_minutes,
  )
  const criticalDuration =
    forecastsAreValid && graph
      ? formatMinuteRange(graph.forecast.min_minutes, graph.forecast.max_minutes)
      : null
  const selectedVisibleIds = visibleNodes
    .filter((node) => isBatchTaskSelectable(node) && selectedTaskIds.has(node.task_id))
    .map((node) => node.task_id)
  const allVisibleSelected =
    visibleSelectableNodes.length > 0 &&
    visibleSelectableNodes.every((node) => selectedTaskIds.has(node.task_id))
  const selectedTaskCount = selectableNodes.filter((node) =>
    selectedTaskIds.has(node.task_id),
  ).length

  const requestKey = (fingerprint: string) => {
    const existing = requestKeys.current.get(fingerprint)
    if (existing) return existing
    const key = crypto.randomUUID()
    requestKeys.current.set(fingerprint, key)
    return key
  }
  const completeRequest = (fingerprint: string) => requestKeys.current.delete(fingerprint)

  const applyProposal = (proposal: ProjectPlanProposalResponse) => {
    if (!graph) return
    setActionError(null)
    const key = `project-plan-apply:${proposal.id}`
    mutations.apply.mutate(
      {
        proposalId: proposal.id,
        body: { expected_version: proposal.base_version, idempotency_key: key },
      },
      {
        onSuccess: () => {
          setRetryableApplyProposalIds((current) => {
            const next = new Set(current)
            next.delete(proposal.id)
            return next
          })
        },
        onError: (error) => {
          setActionError(error)
          if (getApiErrorCode(error) === 'project_plan_apply_incomplete') {
            setRetryableApplyProposalIds((current) => new Set(current).add(proposal.id))
          } else {
            setRetryableApplyProposalIds((current) => {
              const next = new Set(current)
              next.delete(proposal.id)
              return next
            })
          }
        },
      },
    )
  }

  const authorizeSelected = () => {
    if (!graph || selectedVisibleIds.length === 0) return
    const maxParallelTasks = Number(parallelLimit)
    if (!Number.isInteger(maxParallelTasks) || maxParallelTasks < 1) return
    const taskIds = [...selectedVisibleIds].sort()
    const fingerprint = `authorize:${graph.version}:${taskIds.join(',')}:${maxParallelTasks}`
    const key = requestKey(fingerprint)
    setActionError(null)
    mutations.authorize.mutate(
      {
        expected_version: graph.version,
        task_ids: taskIds,
        max_parallel_tasks: maxParallelTasks,
        idempotency_key: key,
      },
      {
        onSuccess: () => completeRequest(fingerprint),
        onError: setActionError,
      },
    )
  }

  const updateBatch = (batch: ProjectPlanBatchResponse, action: BatchOperation) => {
    const fingerprint = `batch:${batch.id}:${batch.version}:${action}`
    const key = requestKey(fingerprint)
    setActionError(null)
    mutations.updateBatch.mutate(
      {
        batchId: batch.id,
        body: { expected_version: batch.version, action, idempotency_key: key },
      },
      {
        onSuccess: () => completeRequest(fingerprint),
        onError: setActionError,
      },
    )
  }

  const toggleTask = (taskId: string) => {
    setSelectedTaskIds((current) => {
      const next = new Set(current)
      if (next.has(taskId)) next.delete(taskId)
      else next.add(taskId)
      return next
    })
  }

  const toggleVisibleSelection = () => {
    const visibleIds = visibleSelectableNodes.map((node) => node.task_id)
    const allSelected = visibleIds.length > 0 && visibleIds.every((id) => selectedTaskIds.has(id))
    setSelectedTaskIds((current) => {
      const next = new Set(current)
      visibleIds.forEach((id) => (allSelected ? next.delete(id) : next.add(id)))
      return next
    })
  }

  if (graphQuery.isLoading) {
    return (
      <div
        className="flex min-h-48 items-center justify-center text-sm text-muted-foreground"
        aria-busy="true"
      >
        {t('projectPlan.loading')}
      </div>
    )
  }
  if (graphQuery.error || !graph) {
    return (
      <ErrorBanner
        error={graphQuery.error}
        fallback={t('projectPlan.loadFailed')}
        onRetry={() => void graphQuery.refetch()}
      />
    )
  }

  return (
    <div className="mx-auto flex w-full max-w-[96rem] flex-col gap-5">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="flex min-w-0 items-start gap-3">
          <div className="mt-0.5 rounded-lg border border-border-subtle bg-muted p-2 text-primary">
            <GitBranch size={19} aria-hidden="true" />
          </div>
          <div className="min-w-0">
            <p className="font-mono text-micro font-semibold uppercase tracking-[0.12em] text-muted-foreground">
              {t('projectPlan.eyebrow')}
            </p>
            <h1 className="mt-1 text-xl font-semibold">{t('projectPlan.title')}</h1>
            <p className="mt-1 max-w-3xl text-sm text-muted-foreground">
              {t('projectPlan.description')}
            </p>
          </div>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          aria-label={t('projectPlan.refresh')}
          disabled={graphQuery.isFetching}
          onClick={() => void graphQuery.refetch()}
        >
          <ArrowClockwise size={15} aria-hidden="true" />
          {graphQuery.isFetching ? t('projectPlan.refreshing') : t('projectPlan.refresh')}
        </Button>
      </header>

      <div className="rounded-lg border border-primary/20 bg-primary/5 p-4 text-sm">
        <p className="font-medium">{t('projectPlan.optionalTitle')}</p>
        <p className="mt-1 text-xs text-muted-foreground">{t('projectPlan.optionalDescription')}</p>
        <p className="mt-2 text-xs text-muted-foreground">{t('projectPlan.copilotHint')}</p>
      </div>

      {actionError ? (
        <div
          role="alert"
          className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive"
        >
          <p>{getApiErrorMessage(actionError, t('projectPlan.actionFailed'))}</p>
          <Button size="sm" variant="outline" onClick={() => void graphQuery.refetch()}>
            {t('projectPlan.refresh')}
          </Button>
        </div>
      ) : null}

      <section aria-labelledby="project-plan-proposals" className="space-y-3">
        <div className="flex flex-wrap items-end justify-between gap-2">
          <div>
            <h2 id="project-plan-proposals" className="text-base font-semibold">
              {t('projectPlan.proposals')}
            </h2>
            <p className="mt-1 text-xs text-muted-foreground">{t('projectPlan.proposalsHint')}</p>
          </div>
          <span className="text-xs text-muted-foreground">
            {t('projectPlan.graphVersion', { version: graph.version })}
          </span>
        </div>
        {graph.proposals.length === 0 ? (
          <Card className="p-4 text-sm text-muted-foreground">{t('projectPlan.noProposals')}</Card>
        ) : (
          <div className="space-y-3">
            {graph.proposals.map((proposal) => (
              <ProposalReview
                key={proposal.id}
                proposal={proposal}
                graphVersion={graph.version}
                canApply={canWritePlan}
                isApplying={mutations.apply.isPending}
                canRetryApply={retryableApplyProposalIds.has(proposal.id)}
                onApply={() => applyProposal(proposal)}
                t={t}
              />
            ))}
          </div>
        )}
      </section>

      <section aria-labelledby="project-plan-graph" className="space-y-4">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <h2 id="project-plan-graph" className="text-base font-semibold">
              {t('projectPlan.graphTitle')}
            </h2>
            <p className="mt-1 text-xs text-muted-foreground">{t('projectPlan.graphHint')}</p>
          </div>
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            <span>{t('projectPlan.phaseFilter')}</span>
            <select
              aria-label={t('projectPlan.phaseFilter')}
              className="h-9 max-w-56 rounded-md border border-input bg-background px-2 text-sm text-foreground"
              value={phaseFilter}
              onChange={(event) => setPhaseFilter(event.target.value)}
            >
              <option value="__all__">{t('projectPlan.allPhases')}</option>
              {phases.map((phase) => (
                <option key={phase || 'no-phase'} value={phase}>
                  {phaseLabel(phase || null, t('projectPlan.noPhase'))}
                </option>
              ))}
            </select>
          </label>
        </div>

        {nodes.length === 0 ? (
          <Card className="space-y-3 p-5">
            <h3 className="font-medium">{t('projectPlan.emptyTitle')}</h3>
            <p className="text-sm text-muted-foreground">{t('projectPlan.emptyDescription')}</p>
            <Link
              to="/projects/$projectId/tasks"
              params={{ projectId }}
              search={{ sort_by: 'updated_at', sort_order: 'desc' }}
              className="inline-flex w-fit items-center rounded-md border border-input px-3 py-2 text-xs font-medium text-primary hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {t('projectPlan.openTasks')}
            </Link>
          </Card>
        ) : (
          <>
            <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
              <MetricCard
                label={t('projectPlan.readyByDependencies')}
                value={String(readyCount)}
                detail={t('projectPlan.readinessIsProjection')}
              />
              <MetricCard
                label={t('projectPlan.structuralWidth')}
                value={String(maxLevelWidth)}
                detail={t('projectPlan.structuralWidthHint')}
              />
              <MetricCard
                label={t('projectPlan.tasksExecutingNow')}
                value={`${activeTasks.length} · ${runningExecutionCount}`}
                detail={t('projectPlan.tasksExecutingDetail')}
              />
              <MetricCard
                label={t('projectPlan.criticalPathDuration')}
                value={criticalDuration ?? t('projectPlan.unknown')}
                detail={
                  criticalDuration
                    ? t('projectPlan.idealEstimateOnly')
                    : t('projectPlan.incompleteEstimateHint')
                }
              />
            </div>
            {forecastsAreValid && graph.forecast.critical_path_task_ids.length > 0 ? (
              <p className="text-xs text-muted-foreground">
                {t('projectPlan.criticalPathTasks')}:{' '}
                {graph.forecast.critical_path_task_ids.map((taskId, index) => {
                  const node = nodes.find((candidate) => candidate.task_id === taskId)
                  return (
                    <span key={taskId}>
                      {index > 0 ? ' → ' : ''}
                      {node ? (
                        <Link
                          to="/tasks/$taskId"
                          params={{ taskId }}
                          className="text-primary hover:underline"
                        >
                          {node.title}
                        </Link>
                      ) : (
                        taskId
                      )}
                    </span>
                  )
                })}
              </p>
            ) : null}

            {canManageBatch ? (
              <Card className="space-y-3 p-4">
                <div className="flex flex-wrap items-end justify-between gap-3">
                  <div>
                    <h3 className="text-sm font-semibold">{t('projectPlan.authorizeTitle')}</h3>
                    <p className="mt-1 max-w-3xl text-xs text-muted-foreground">
                      {t('projectPlan.authorizeHint')}
                    </p>
                    <p className="mt-1 max-w-3xl text-xs text-muted-foreground">
                      {t('projectPlan.blockedTasksIncluded')}
                    </p>
                  </div>
                  <div className="flex flex-wrap items-end gap-2">
                    <label className="flex flex-col gap-1 text-xs text-muted-foreground">
                      <span>{t('projectPlan.maxParallelTasks')}</span>
                      <input
                        aria-label={t('projectPlan.maxParallelTasks')}
                        type="number"
                        min="1"
                        step="1"
                        value={parallelLimit}
                        onChange={(event) => setParallelLimit(event.target.value)}
                        className="h-9 w-28 rounded-md border border-input bg-background px-2 text-sm text-foreground"
                      />
                    </label>
                    <Button
                      type="button"
                      size="sm"
                      disabled={
                        selectedVisibleIds.length === 0 ||
                        !Number.isInteger(Number(parallelLimit)) ||
                        Number(parallelLimit) < 1 ||
                        mutations.authorize.isPending
                      }
                      onClick={authorizeSelected}
                    >
                      {mutations.authorize.isPending
                        ? t('projectPlan.authorizing')
                        : t('projectPlan.authorizeSelected', { count: selectedVisibleIds.length })}
                    </Button>
                  </div>
                </div>
                <div className="flex flex-wrap items-center gap-3 text-xs">
                  <button
                    type="button"
                    aria-pressed={allVisibleSelected}
                    className="text-primary underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    onClick={toggleVisibleSelection}
                  >
                    {t('projectPlan.toggleVisibleSelection')}
                  </button>
                  <button
                    type="button"
                    className="text-muted-foreground underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    onClick={() => setSelectedTaskIds(new Set())}
                  >
                    {t('projectPlan.clearSelection')}
                  </button>
                  <span className="text-muted-foreground">
                    {t('projectPlan.selectedCount', { count: selectedTaskCount })}
                  </span>
                  <span className="text-muted-foreground">
                    {t('projectPlan.workUnitSlotsSeparate')}
                  </span>
                </div>
              </Card>
            ) : (
              <p className="rounded-md border border-border-subtle bg-muted/30 p-3 text-xs text-muted-foreground">
                {t('projectPlan.adminOnlyAuthorize')}
              </p>
            )}

            {visibleNodes.length === 0 ? (
              <Card className="p-4 text-sm text-muted-foreground">
                {t('projectPlan.noPhaseTasks')}
              </Card>
            ) : (
              <div className="space-y-3">
                <div
                  role="region"
                  aria-label={t('projectPlan.graphAccessibleLabel')}
                  className="overflow-auto rounded-lg border border-border-subtle bg-background p-2"
                >
                  <div className="relative" style={{ width: layout.width, height: layout.height }}>
                    <svg
                      aria-hidden="true"
                      className="absolute inset-0 overflow-visible"
                      width={layout.width}
                      height={layout.height}
                      viewBox={`0 0 ${layout.width} ${layout.height}`}
                    >
                      <defs>
                        <marker
                          id="project-plan-arrow"
                          viewBox="0 0 10 10"
                          refX="8"
                          refY="5"
                          markerWidth="5"
                          markerHeight="5"
                          orient="auto-start-reverse"
                        >
                          <path d="M 0 0 L 10 5 L 0 10 z" fill="hsl(var(--primary))" />
                        </marker>
                        <marker
                          id="project-plan-arrow-inferred"
                          viewBox="0 0 10 10"
                          refX="8"
                          refY="5"
                          markerWidth="5"
                          markerHeight="5"
                          orient="auto-start-reverse"
                        >
                          <path d="M 0 0 L 10 5 L 0 10 z" fill="hsl(var(--muted-foreground))" />
                        </marker>
                      </defs>
                      {visibleEdges.map((edge) => {
                        const path = edgePath(edge, layout.positions)
                        return path ? (
                          <path
                            key={`${edge.predecessor_key}:${edge.successor_key}`}
                            data-project-plan-edge="true"
                            d={path}
                            fill="none"
                            stroke={
                              edge.inferred ? 'hsl(var(--muted-foreground))' : 'hsl(var(--primary))'
                            }
                            strokeDasharray={edge.inferred ? '5 4' : undefined}
                            markerEnd={
                              edge.inferred
                                ? 'url(#project-plan-arrow-inferred)'
                                : 'url(#project-plan-arrow)'
                            }
                            strokeWidth="2"
                          />
                        ) : null
                      })}
                    </svg>
                    {visibleNodes.map((node) => {
                      const position = layout.positions.get(node.key)
                      if (!position) return null
                      return (
                        <TaskNodeCard
                          key={node.key}
                          node={node}
                          projectId={projectId}
                          x={position.x}
                          y={position.y}
                          selected={
                            isBatchTaskSelectable(node) && selectedTaskIds.has(node.task_id)
                          }
                          canManage={canManageBatch}
                          canSelect={isBatchTaskSelectable(node)}
                          sources={graph.sources}
                          t={t}
                          onToggle={toggleTask}
                        />
                      )
                    })}
                  </div>
                </div>
                <div
                  className="flex flex-wrap gap-x-4 gap-y-1 text-micro text-muted-foreground"
                  aria-label={t('projectPlan.edgeLegend')}
                >
                  <span>
                    <span className="mr-1 inline-block w-5 border-t-2 border-primary align-middle" />
                    {t('projectPlan.explicit')}
                  </span>
                  <span>
                    <span className="mr-1 inline-block w-5 border-t-2 border-dashed border-muted-foreground align-middle" />
                    {t('projectPlan.inferred')}
                  </span>
                </div>
              </div>
            )}

            <section aria-labelledby="project-plan-dependencies" className="space-y-2">
              <h3 id="project-plan-dependencies" className="text-sm font-semibold">
                {t('projectPlan.dependencies')}
              </h3>
              <EdgeTable
                edges={visibleEdges}
                nodes={nodes}
                sources={graph.sources}
                noDependencies={t('projectPlan.noDependencies')}
                t={t}
              />
            </section>

            <SourceList sources={graph.sources} title={t('projectPlan.provenance')} t={t} />
          </>
        )}
      </section>

      <section aria-labelledby="project-plan-batches" className="space-y-3">
        <div>
          <h2 id="project-plan-batches" className="text-base font-semibold">
            {t('projectPlan.batches')}
          </h2>
          <p className="mt-1 text-xs text-muted-foreground">{t('projectPlan.batchesHint')}</p>
        </div>
        {graph.batches.length === 0 ? (
          <Card className="p-4 text-sm text-muted-foreground">{t('projectPlan.noBatches')}</Card>
        ) : (
          <div className="grid gap-3 xl:grid-cols-2">
            {graph.batches.map((batch) => {
              const actions = getBatchActions(batch.state)
              return (
                <Card key={batch.id} className="space-y-3 p-4">
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="min-w-0">
                      <h3 className="break-all font-mono text-xs font-semibold">{batch.id}</h3>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {t('projectPlan.batchSummary', {
                          count: batch.task_ids.length,
                          parallel: batch.max_parallel_tasks,
                        })}
                      </p>
                    </div>
                    <span className="rounded-full border border-border-subtle bg-muted px-2 py-0.5 text-xs">
                      {humanize(batch.state)}
                    </span>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {actions.map((action) => (
                      <Button
                        key={action}
                        type="button"
                        size="sm"
                        variant={action === 'revoke' ? 'outline' : 'secondary'}
                        disabled={!canManageBatch || mutations.updateBatch.isPending}
                        onClick={() => updateBatch(batch, action)}
                      >
                        {action === 'pause' ? <Pause size={14} aria-hidden="true" /> : null}
                        {action === 'resume' ? <Play size={14} aria-hidden="true" /> : null}
                        {action === 'revoke' ? <Prohibit size={14} aria-hidden="true" /> : null}
                        {t(`projectPlan.batchAction.${action}`)}
                      </Button>
                    ))}
                    {!canManageBatch && actions.length > 0 ? (
                      <span className="self-center text-xs text-muted-foreground">
                        {t('projectPlan.adminOnlyBatch')}
                      </span>
                    ) : null}
                  </div>
                </Card>
              )
            })}
          </div>
        )}
      </section>
    </div>
  )
}

function MetricCard({ label, value, detail }: { label: string; value: string; detail: string }) {
  return (
    <Card className="p-3">
      <p className="text-xs text-muted-foreground">{label}</p>
      <p className="mt-1 text-lg font-semibold">{value}</p>
      <p className="mt-1 text-micro text-muted-foreground">{detail}</p>
    </Card>
  )
}
