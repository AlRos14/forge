import { describe, expect, it } from 'vitest'
import { navigationItemsForSection } from './app-shell'

describe('application shell navigation contract', () => {
  it('exposes target Project navigation without retired workspaces', () => {
    expect(navigationItemsForSection('main')).toEqual([])
    expect(navigationItemsForSection('project').map(({ key, to }) => [key, to])).toEqual([
      ['board', '/projects/$projectId/board'],
      ['tasks', '/projects/$projectId/tasks'],
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
