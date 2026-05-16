import { describe, expect, it } from 'vitest'
import {
  EXECUTION_CONFIG_STORAGE_KEY,
  readRecentExecutionSelections,
  resolveExecutionOverrides,
  saveRecentExecutionSelection,
} from '@/lib/execution-config-storage'

class MemoryStorage implements Pick<Storage, 'getItem' | 'setItem'> {
  private readonly values = new Map<string, string>()

  getItem(key: string): string | null {
    return this.values.get(key) ?? null
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value)
  }
}

describe('execution config storage', () => {
  it('omits default selections from execution overrides', () => {
    expect(
      resolveExecutionOverrides({
        modelId: null,
        reasoningEffort: null,
        permissionPolicy: null,
      }),
    ).toBeUndefined()

    expect(
      resolveExecutionOverrides({
        modelId: 'gpt-5.4',
        reasoningEffort: null,
        permissionPolicy: 'supervised',
      }),
    ).toEqual({
      model_id: 'gpt-5.4',
      permission_policy: 'supervised',
    })
  })

  it('keeps recent models per profile as an LRU list', () => {
    const storage = new MemoryStorage()
    for (const modelId of ['a', 'b', 'c', 'd', 'e', 'f', 'c']) {
      saveRecentExecutionSelection(
        'profile-1',
        { modelId, reasoningEffort: 'high', permissionPolicy: 'auto' },
        storage,
      )
    }

    const recent = readRecentExecutionSelections(storage)['profile-1']
    expect(recent.recentModels.map((model) => model.modelId)).toEqual(['c', 'f', 'e', 'd', 'b'])
    expect(recent.lastModelId).toBe('c')
    expect(recent.lastReasoningEffort).toBe('high')
  })

  it('falls back to empty state when stored JSON is corrupted', () => {
    const storage = new MemoryStorage()
    storage.setItem(EXECUTION_CONFIG_STORAGE_KEY, '{not-json')

    expect(readRecentExecutionSelections(storage)).toEqual({})
  })
})
