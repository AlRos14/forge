export type ParsedGitRemote = {
  platform: 'github' | 'gitea'
  baseUrl: string
  owner: string
  repo: string
}

function normalizeRepoName(repo: string): string {
  return repo.replace(/\.git$/i, '')
}

function parsedFromHostAndPath(host: string, path: string): ParsedGitRemote | null {
  const parts = path.replace(/^\/+/, '').split('/').filter(Boolean)
  if (parts.length < 2) return null

  const owner = parts[parts.length - 2]
  const repo = normalizeRepoName(parts[parts.length - 1])
  if (!owner || !repo) return null

  const normalizedHost = host.toLowerCase()
  if (normalizedHost === 'github.com') {
    return {
      platform: 'github',
      baseUrl: 'https://api.github.com',
      owner,
      repo,
    }
  }

  return {
    platform: 'gitea',
    baseUrl: `https://${host}`,
    owner,
    repo,
  }
}

export function parseGitRemoteUrl(remoteUrl: string | null | undefined): ParsedGitRemote | null {
  const value = remoteUrl?.trim().replace(/^git\+/, '') ?? ''
  if (!value) return null
  if (value.startsWith('/') || value.startsWith('~') || /^[A-Za-z]:[\\/]/.test(value)) {
    return null
  }

  try {
    const parsed = new URL(value)
    return parsedFromHostAndPath(parsed.hostname, parsed.pathname)
  } catch {
    // Continue with SCP-like SSH remotes such as git@github.com:org/repo.git.
  }

  const scpMatch = /^(?:[^@]+@)?([^:]+):(.+)$/.exec(value)
  if (scpMatch) {
    return parsedFromHostAndPath(scpMatch[1], scpMatch[2])
  }

  try {
    const parsed = new URL(`https://${value}`)
    return parsedFromHostAndPath(parsed.hostname, parsed.pathname)
  } catch {
    return null
  }
}
