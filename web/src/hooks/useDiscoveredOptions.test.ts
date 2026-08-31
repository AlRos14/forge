import { describe, expect, it } from 'vitest'
import { getReasoningOptionsForModel, normalizeDiscoveredOptions } from './useDiscoveredOptions'

describe('normalizeDiscoveredOptions', () => {
  it('uses adapter-provided reasoning efforts for each model', () => {
    const options = normalizeDiscoveredOptions({
      models: ['gpt-5.6-sol', 'gpt-5.6-luna'],
      cli_specific: {
        reasoning_efforts: ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'],
        model_reasoning_efforts: {
          'gpt-5.6-sol': ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'],
          'gpt-5.6-luna': ['low', 'medium', 'high', 'xhigh', 'max'],
        },
      },
    })

    expect(getReasoningOptionsForModel(options, 'gpt-5.6-sol').map((entry) => entry.id)).toEqual([
      'low',
      'medium',
      'high',
      'xhigh',
      'max',
      'ultra',
    ])
    expect(getReasoningOptionsForModel(options, 'gpt-5.6-luna').map((entry) => entry.id)).toEqual([
      'low',
      'medium',
      'high',
      'xhigh',
      'max',
    ])
  })

  it('preserves an explicit empty effort list for models without reasoning controls', () => {
    const options = normalizeDiscoveredOptions({
      models: ['claude-fable-5', 'claude-haiku-4-5'],
      cli_specific: {
        reasoning_efforts: ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'],
        model_reasoning_efforts: {
          'claude-fable-5': ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'],
          'claude-haiku-4-5': [],
        },
      },
    })

    expect(getReasoningOptionsForModel(options, 'claude-haiku-4-5')).toEqual([])
    expect(getReasoningOptionsForModel(options, 'claude-fable-5').at(-1)).toEqual({
      id: 'ultracode',
      label: 'Ultracode',
    })
  })

  it('labels cursor, grok, and gemini model providers', () => {
    const options = normalizeDiscoveredOptions({
      models: ['cursor-grok-4.6-medium-fast', 'composer-2.5', 'gemini-3.7-flash-high'],
    })

    expect(options.models.map((model) => [model.id, model.provider])).toEqual([
      ['cursor-grok-4.6-medium-fast', 'xAI'],
      ['composer-2.5', 'Cursor'],
      ['gemini-3.7-flash-high', 'Google'],
    ])
    expect(options.models.every((model) => model.reasoningOptions.length === 0)).toBe(true)
  })

  it('does not invent reasoning controls when an adapter omits the capability', () => {
    const options = normalizeDiscoveredOptions({ models: ['custom-model'] })

    expect(getReasoningOptionsForModel(options, 'custom-model')).toEqual([])
    expect(options.permissionPolicies).toEqual([])
  })

  it('preserves only permission policies advertised by the adapter', () => {
    const options = normalizeDiscoveredOptions({
      models: ['gemini-3.1-pro-preview'],
      permission_policies: ['auto', 'supervised'],
    })

    expect(options.permissionPolicies).toEqual(['auto', 'supervised'])
  })
})
