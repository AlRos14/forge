import { useEffect } from 'react'
import type { QueryClient } from '@tanstack/react-query'
import { qk } from '@/api/query-keys'
import type { ForgeEvent, PublicEventType } from '@/types/generated'

type SsePayload = ForgeEvent

type BrowserEvents = { dispatch: (name: string, detail: SsePayload) => void }

function parseSseData(raw: string): SsePayload | undefined {
  try {
    return JSON.parse(raw) as SsePayload
  } catch {
    return undefined
  }
}

function invalidateAllActiveQueries(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({
    predicate: () => true,
    refetchType: 'active',
  })
}

function invalidateProjectTaskLists(queryClient: QueryClient, projectId?: string): void {
  if (projectId) {
    void queryClient.invalidateQueries({ queryKey: qk.projectTasks(projectId) })
    return
  }
  void queryClient.invalidateQueries({
    predicate: (query) => query.queryKey[0] === 'projects' && query.queryKey[2] === 'tasks',
  })
}

export function routeSsePayload(
  payload: SsePayload,
  queryClient: QueryClient,
  browserEvents: BrowserEvents,
): void {
  const eventType = payload.event_type

  const facts = payload.payload ?? (payload as unknown as Record<string, unknown>)

  if (eventType === 'events.resync_required') {
    invalidateAllActiveQueries(queryClient)
    return
  }

  if (eventType === 'operations.status_changed') {
    void queryClient.invalidateQueries({ queryKey: qk.operationsStatus })
  }

  if (eventType === 'project_hook.run_changed' && typeof facts.project_id === 'string') {
    void queryClient.invalidateQueries({ queryKey: qk.projectHookRuns(facts.project_id) })
  }

  if (eventType === 'task.lifecycle_changed') {
    const taskId = typeof facts.task_id === 'string' ? facts.task_id : payload.entity_id
    void queryClient.invalidateQueries({ queryKey: qk.task(taskId) })
    void queryClient.invalidateQueries({ queryKey: qk.taskDetail(taskId) })
    invalidateProjectTaskLists(queryClient)
    void queryClient.invalidateQueries({ queryKey: qk.gates(taskId) })
  }

  if (eventType.startsWith('gate.')) {
    const taskId = typeof facts.task_id === 'string' ? facts.task_id : payload.scope_id
    if (taskId) {
      void queryClient.invalidateQueries({ queryKey: qk.task(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.taskDetail(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.gates(taskId) })
    } else {
      invalidateAllActiveQueries(queryClient)
    }
  }

  if (eventType.startsWith('execution.') && payload.entity_type === 'execution') {
    const taskId = payload.scope_type === 'task' ? payload.scope_id : undefined
    if (taskId) {
      void queryClient.invalidateQueries({ queryKey: qk.task(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.taskDetail(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.executions(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.taskDiff(taskId) })
      invalidateProjectTaskLists(queryClient)
    }
    void queryClient.invalidateQueries({ queryKey: qk.execution(payload.entity_id) })
  }

  if (eventType.startsWith('validation_run.') || eventType === 'evidence.created') {
    const taskId = payload.scope_type === 'task' ? payload.scope_id : undefined
    if (taskId) {
      void queryClient.invalidateQueries({ queryKey: qk.validationRuns(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.taskDetail(taskId) })
    } else {
      invalidateAllActiveQueries(queryClient)
    }
  }

  if (eventType.startsWith('project.')) {
    void queryClient.invalidateQueries({ queryKey: qk.project(payload.entity_id) })
    void queryClient.invalidateQueries({ queryKey: qk.projects })
  }

  if (['artifact.created', 'message.created', 'handoff.created', 'handoff.status_changed', 'proposal.created', 'proposal.withdrawn', 'decision.recorded'].includes(eventType)) {
    const taskId = payload.scope_type === 'task' ? payload.scope_id : undefined
    if (taskId) void queryClient.invalidateQueries({ queryKey: qk.taskDetail(taskId) })
  }

  if (eventType === 'notification.created') {
    void queryClient.invalidateQueries({
      predicate: (query) => String(query.queryKey[0]) === 'notifications',
    })
    browserEvents.dispatch('forge:notification-created', { ...payload, ...facts })
  }
}

export function useSSE(queryClient: QueryClient, accessToken: string | null): void {
  useEffect(() => {
    if (!accessToken) return

    let cancelled = false
    let source: EventSource | null = null
    let backoffMs = 1000
    let backoffTimer: ReturnType<typeof setTimeout> | null = null

    const handleEvent = (event: MessageEvent<string>) => {
      const payload = parseSseData(event.data)
      if (!payload) return
      routeSsePayload(payload, queryClient, {
        dispatch: (name, detail) => {
          window.dispatchEvent(new CustomEvent(name, { detail }))
        },
      })
    }

    const connect = () => {
      if (backoffTimer) {
        clearTimeout(backoffTimer)
        backoffTimer = null
      }
      source = new EventSource(`/api/v1/events?token=${encodeURIComponent(accessToken)}`)

      // Listen on generic message (unnamed events)
      source.onmessage = handleEvent

      // Also listen on known named event types so we catch both
      const namedEvents: PublicEventType[] = [
        'task.lifecycle_changed',
        'gate.created',
        'gate.policy_revised',
        'gate.evaluated',
        'execution.started',
        'execution.completed',
        'execution.failed',
        'execution.cancelled',
        'execution.stalled',
        'validation_run.started',
        'validation_run.completed',
        'evidence.created',
        'artifact.created',
        'message.created',
        'handoff.created',
        'handoff.status_changed',
        'proposal.created',
        'proposal.withdrawn',
        'decision.recorded',
        'project.created',
        'project.updated',
        'project.deleted',
        'project.paused',
        'project.resumed',
        'project_hook.run_changed',
        'notification.created',
        'events.resync_required',
        'operations.status_changed',
      ]
      for (const name of namedEvents) {
        source.addEventListener(name, handleEvent as EventListener)
      }

      source.onerror = () => {
        source?.close()
        if (cancelled) return
        backoffTimer = setTimeout(() => {
          if (!cancelled) {
            backoffMs = Math.min(backoffMs * 2, 30_000)
            connect()
          }
        }, backoffMs)
      }

      source.onopen = () => {
        backoffMs = 1000
      }
    }

    connect()
    return () => {
      cancelled = true
      if (backoffTimer) clearTimeout(backoffTimer)
      source?.close()
    }
  }, [queryClient, accessToken])
}
