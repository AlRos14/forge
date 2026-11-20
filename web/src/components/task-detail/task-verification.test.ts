import { describe, expect, it } from 'vitest'
import type { ValidationComparisonClass } from '@/types/generated'
import {
  canAnswerDecisionOption,
  canManageValidationCatalog,
  comparisonBadge,
  readRequiredChecks,
  validationRepositoryLabel,
} from './task-verification'

describe('comparison badge mapping', () => {
  const classes: ValidationComparisonClass[] = [
    'regression',
    'pre_existing_failure',
    'fixed',
    'both_pass',
    'inconclusive',
  ]
  it('maps every classification to a distinct label with an explanation', () => {
    const labels = classes.map((value) => comparisonBadge(value).label)
    expect(new Set(labels).size).toBe(classes.length)
    for (const value of classes) expect(comparisonBadge(value).explanation.length).toBeGreaterThan(10)
  })
  it('only a regression is shown as destructive', () => {
    expect(comparisonBadge('regression').variant).toBe('destructive')
    expect(comparisonBadge('pre_existing_failure').variant).not.toBe('destructive')
  })
})

describe('catalog permission', () => {
  it('lets owner and admin manage, members read only', () => {
    expect(canManageValidationCatalog('owner')).toBe(true)
    expect(canManageValidationCatalog('admin')).toBe(true)
    expect(canManageValidationCatalog('member')).toBe(false)
    expect(canManageValidationCatalog(undefined)).toBe(false)
  })
})

describe('decision authority', () => {
  it('requires owner/admin for scope and authority options', () => {
    expect(canAnswerDecisionOption('none', false)).toBe(true)
    expect(canAnswerDecisionOption('scope', false)).toBe(false)
    expect(canAnswerDecisionOption('authority', true)).toBe(true)
  })
})

describe('required checks', () => {
  it('reads schema v2 required_checks and ignores malformed entries', () => {
    expect(
      readRequiredChecks({
        required_checks: [
          { check_key: 'unit', definition_digest: 'abc', required_outcome: 'passed' },
          { nope: true },
          'x',
        ],
      }),
    ).toEqual([{ check_key: 'unit', definition_digest: 'abc', required_outcome: 'passed' }])
    expect(readRequiredChecks({})).toEqual([])
  })
})

describe('validation repository label', () => {
  it('multirepo_result_validation_labels_repo', () => {
    expect(validationRepositoryLabel({ repo_id: 'repo-api' })).toBe('Repository repo-api')
  })
})
