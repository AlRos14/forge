import { describe, expect, it } from 'vitest'
import { canonicalJson } from './api'

describe('Project workbench API', () => {
  it('matches the server recursive key-sorted canonical JSON contract', () => {
    expect(
      canonicalJson({ z: 1, nested: { beta: 2, alpha: 1 }, list: [{ d: 4, c: 3 }] }),
    ).toBe('{"list":[{"c":3,"d":4}],"nested":{"alpha":1,"beta":2},"z":1}')
  })
})
