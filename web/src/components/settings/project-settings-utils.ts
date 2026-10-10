import { ApiError } from '@/api/client'
import { getApiErrorMessage } from '@/lib/api-error'
import type { Repo } from '@/types/generated'
import type { RepoFormState } from '@/components/settings/RepoForm'

export function settingsErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError && error.status === 409) return 'Reload and retry'
  return getApiErrorMessage(error, fallback)
}

export function repoFormFromRepo(repo: Repo): RepoFormState {
  return {
    source_mode: repo.local_path ? 'local' : 'remote',
    name: repo.name,
    local_path: repo.local_path ?? '',
    remote_url: repo.remote_url,
    default_branch: repo.default_branch || 'main',
    work_mode: repo.work_mode,
    pr_provider: repo.pr_provider_status?.provider_type ?? repo.pr_provider ?? 'github',
    pr_base_url: '',
    pr_token: '',
    pr_polling_interval_seconds: String(repo.pr_provider_status?.polling_interval_seconds ?? 60),
  }
}

export function repoSource(repo: Repo): string {
  return repo.remote_url
}

export function formatDuration(ms: number | null): string {
  if (ms === null) return '—'
  if (ms < 1000) return `${ms}ms`
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
  return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`
}

export function formatTokens(n: number): string {
  return n.toLocaleString()
}

export function formatCost(usd: number | null): string {
  if (usd === null) return '—'
  return `$${usd.toFixed(2)}`
}

export function formatRate(rate: number): string {
  return `${(rate * 100).toFixed(1)}%`
}
