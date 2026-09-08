import type { JsonObject } from './types'

export function cliCommandFromConfig(config: JsonObject | undefined): string {
  const value = config?.base_command_override
  return typeof value === 'string' ? value : ''
}

export function envStringFromConfig(config: JsonObject | undefined, key: string): string {
  const env = config?.env
  if (!env || typeof env !== 'object' || Array.isArray(env)) return ''
  const value = (env as Record<string, unknown>)[key]
  return typeof value === 'string' ? value : ''
}

export function configWithHarnessLaunch(
  config: JsonObject | undefined,
  input: { command: string; envPatch?: Record<string, string | null> },
): JsonObject {
  const next: JsonObject = { ...(config ?? {}) }
  const trimmed = input.command.trim()
  if (trimmed) {
    next.base_command_override = trimmed
  } else {
    delete next.base_command_override
  }
  if (!input.envPatch) {
    return next
  }
  const env: Record<string, string> = {}
  const existing = next.env
  if (existing && typeof existing === 'object' && !Array.isArray(existing)) {
    for (const [key, value] of Object.entries(existing as Record<string, unknown>)) {
      if (typeof value === 'string') env[key] = value
    }
  }
  for (const [key, value] of Object.entries(input.envPatch)) {
    const trimmedValue = value?.trim() ?? ''
    if (trimmedValue) env[key] = trimmedValue
    else delete env[key]
  }
  if (Object.keys(env).length > 0) {
    next.env = env
  } else {
    delete next.env
  }
  return next
}

export function configsEqual(left: JsonObject, right: JsonObject): boolean {
  return JSON.stringify(left) === JSON.stringify(right)
}

export function cliCommandPlaceholder(executorType: string): string {
  if (executorType === 'codex') return 'codex2'
  if (executorType === 'claude_code') return 'claude'
  if (executorType === 'cursor') return 'cursor-agent'
  if (executorType === 'opencode') return 'opencode'
  if (executorType === 'gemini') return 'gemini'
  return '/custom/path/to/cli'
}
