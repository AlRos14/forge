import { describe, expect, it } from 'vitest'
import { navigationItemsForSection } from './app-shell'

describe('application shell navigation contract', () => {
  it('places the canonical Main Chat before Project navigation', () => {
    expect(navigationItemsForSection('main').map(({ key, to }) => [key, to])).toEqual([
      ['mainChat', '/chat'],
    ])
    expect(navigationItemsForSection('project').map(({ key, to }) => [key, to])).toEqual([
      ['overview', '/projects/$projectId/overview'],
      ['board', '/projects/$projectId/board'],
      ['tasks', '/projects/$projectId/tasks'],
      ['agentWorkspace', '/projects/$projectId/chat'],
      ['settings', '/projects/$projectId/settings'],
    ])
  })

  it('keeps Agent Settings and Forge Settings distinct', () => {
    const global = navigationItemsForSection('global').map(({ key, to }) => [key, to])
    expect(global).toContainEqual(['agentSettings', '/agents'])
    expect(global).toContainEqual(['forgeSettings', '/settings'])
    expect(global.flat()).not.toContain('/agents/federated')
  })
})
