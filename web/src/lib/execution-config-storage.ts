export const EXECUTION_CONFIG_STORAGE_KEY = 'forge:execution-config-recent'

export type ExecutionConfigSelection = {
  modelId: string | null
  reasoningEffort: string | null
  permissionPolicy: string | null
}

export type ExecutionOverrides = {
  model_id?: string
  reasoning_effort?: string
  permission_policy?: string
}

export type RecentModel = {
  modelId: string
  usedAt: string
}

export type RecentExecutionSelection = {
  lastModelId: string | null
  lastReasoningEffort: string | null
  lastPermissionPolicy: string | null
  recentModels: RecentModel[]
}

export type RecentExecutionSelections = Record<string, RecentExecutionSelection>

type StorageLike = Pick<Storage, 'getItem' | 'setItem'>

const MAX_RECENT_MODELS = 5

function browserStorage(): StorageLike | undefined {
  try {
    return window.localStorage
  } catch {
    return undefined
  }
}

function asStringOrNull(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null
}

function normalizeEntry(value: unknown): RecentExecutionSelection | null {
  if (!value || typeof value !== 'object') return null
  const record = value as Record<string, unknown>
  const recentModels = Array.isArray(record.recentModels)
    ? record.recentModels
        .map((item): RecentModel | null => {
          if (!item || typeof item !== 'object') return null
          const model = item as Record<string, unknown>
          const modelId = asStringOrNull(model.modelId)
          const usedAt = asStringOrNull(model.usedAt)
          return modelId && usedAt ? { modelId, usedAt } : null
        })
        .filter((item): item is RecentModel => Boolean(item))
        .slice(0, MAX_RECENT_MODELS)
    : []

  return {
    lastModelId: asStringOrNull(record.lastModelId),
    lastReasoningEffort: asStringOrNull(record.lastReasoningEffort),
    lastPermissionPolicy: asStringOrNull(record.lastPermissionPolicy),
    recentModels,
  }
}

export function readRecentExecutionSelections(
  storage: StorageLike | undefined = browserStorage(),
): RecentExecutionSelections {
  if (!storage) return {}
  try {
    const raw = storage.getItem(EXECUTION_CONFIG_STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== 'object') return {}

    const result: RecentExecutionSelections = {}
    for (const [profileId, value] of Object.entries(parsed)) {
      if (!profileId) continue
      const normalized = normalizeEntry(value)
      if (normalized) result[profileId] = normalized
    }
    return result
  } catch {
    return {}
  }
}

export function getRecentExecutionSelection(
  agentId: string | null | undefined,
  storage?: StorageLike,
): RecentExecutionSelection | null {
  if (!agentId) return null
  return readRecentExecutionSelections(storage)[agentId] ?? null
}

export function saveRecentExecutionSelection(
  agentId: string | null | undefined,
  selection: ExecutionConfigSelection,
  storage: StorageLike | undefined = browserStorage(),
): void {
  if (!agentId || !storage) return

  const current = readRecentExecutionSelections(storage)
  const previous = current[agentId]
  const now = new Date().toISOString()
  const recentModels = selection.modelId
    ? [
        { modelId: selection.modelId, usedAt: now },
        ...(previous?.recentModels ?? []).filter((model) => model.modelId !== selection.modelId),
      ].slice(0, MAX_RECENT_MODELS)
    : (previous?.recentModels ?? []).slice(0, MAX_RECENT_MODELS)

  current[agentId] = {
    lastModelId: selection.modelId,
    lastReasoningEffort: selection.reasoningEffort,
    lastPermissionPolicy: selection.permissionPolicy,
    recentModels,
  }

  try {
    storage.setItem(EXECUTION_CONFIG_STORAGE_KEY, JSON.stringify(current))
  } catch {
    // Persistence is best-effort; execution still works without localStorage.
  }
}

export function resolveExecutionOverrides(
  selection: ExecutionConfigSelection,
): ExecutionOverrides | undefined {
  const overrides: ExecutionOverrides = {}
  if (selection.modelId) overrides.model_id = selection.modelId
  if (selection.reasoningEffort) overrides.reasoning_effort = selection.reasoningEffort
  if (selection.permissionPolicy) overrides.permission_policy = selection.permissionPolicy
  return Object.keys(overrides).length > 0 ? overrides : undefined
}
