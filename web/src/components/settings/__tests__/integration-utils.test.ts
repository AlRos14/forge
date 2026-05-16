import { describe, expect, it } from 'vitest'

import { parseGitRemoteUrl } from '@/components/settings/integration-utils'

describe('parseGitRemoteUrl', () => {
  it('parses GitHub HTTPS remotes', () => {
    expect(parseGitRemoteUrl('https://github.com/acme/forge.git')).toEqual({
      platform: 'github',
      baseUrl: 'https://api.github.com',
      owner: 'acme',
      repo: 'forge',
    })
  })

  it('parses GitHub SSH remotes', () => {
    expect(parseGitRemoteUrl('git@github.com:acme/forge.git')).toEqual({
      platform: 'github',
      baseUrl: 'https://api.github.com',
      owner: 'acme',
      repo: 'forge',
    })
  })

  it('parses self-hosted Git remotes as Gitea defaults', () => {
    expect(parseGitRemoteUrl('ssh://git@gitea.example.com/acme/forge.git')).toEqual({
      platform: 'gitea',
      baseUrl: 'https://gitea.example.com',
      owner: 'acme',
      repo: 'forge',
    })
  })

  it('returns null when the remote cannot identify an owner and repo', () => {
    expect(parseGitRemoteUrl('/Users/acme/forge')).toBeNull()
  })
})
