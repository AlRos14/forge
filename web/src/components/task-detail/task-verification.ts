import type {
  AuthorityEffect,
  HumanDecisionResponse,
  HumanDecisionState,
  ValidationCheckRequestState,
  ValidationComparisonClass,
} from '@/types/generated'

export type ComparisonBadge = {
  label: string
  variant: 'outline' | 'destructive' | 'secondary' | 'default'
  explanation: string
}

export const comparisonBadges: Record<ValidationComparisonClass, ComparisonBadge> = {
  regression: {
    label: 'Regression',
    variant: 'destructive',
    explanation: 'The check passed on the baseline but fails with this change. The change broke it.',
  },
  pre_existing_failure: {
    label: 'Already failing',
    variant: 'secondary',
    explanation:
      'The check fails on the baseline and still fails with this change. The failure was there before this work.',
  },
  fixed: {
    label: 'Fixed',
    variant: 'default',
    explanation: 'The check failed on the baseline and passes with this change.',
  },
  both_pass: {
    label: 'Passes both',
    variant: 'outline',
    explanation: 'The check passes on the baseline and with this change.',
  },
  inconclusive: {
    label: 'Inconclusive',
    variant: 'secondary',
    explanation:
      'The baseline or candidate run did not finish with a usable result, so no conclusion is drawn.',
  },
}

export function comparisonBadge(classification: ValidationComparisonClass): ComparisonBadge {
  return comparisonBadges[classification]
}

export const checkRequestStateLabels: Record<ValidationCheckRequestState, string> = {
  queued: 'Queued',
  running: 'Running',
  completed: 'Completed',
  cancelled: 'Cancelled',
  failed_to_start: 'Could not start',
}

export const decisionStateLabels: Record<HumanDecisionState, string> = {
  open: 'Waiting for your answer',
  answered: 'Answered',
  superseded: 'Superseded',
  cancelled: 'Cancelled',
}

export const authorityEffectLabels: Record<AuthorityEffect, string> = {
  none: 'No authority change',
  scope: 'Changes scope',
  authority: 'Changes authority',
}

export function authorityEffectVariant(effect: AuthorityEffect): 'outline' | 'destructive' {
  return effect === 'none' ? 'outline' : 'destructive'
}

export function isDecisionOpen(decision: Pick<HumanDecisionResponse, 'state'>): boolean {
  return decision.state === 'open'
}

/** Owner/admin required when the option, or any choice in the decision, may change scope/authority. */
export function decisionOptionNeedsPrivilege(effect: AuthorityEffect): boolean {
  return effect !== 'none'
}

export function canAnswerDecisionOption(effect: AuthorityEffect, isOwnerOrAdmin: boolean): boolean {
  return !decisionOptionNeedsPrivilege(effect) || isOwnerOrAdmin
}

/** Catalog management is Project owner/admin only; members read. */
export function canManageValidationCatalog(role: string | undefined): boolean {
  return role === 'owner' || role === 'admin'
}

export type RequiredCheck = { check_key: string; definition_digest: string; required_outcome: string }

export function readRequiredChecks(policy: Record<string, unknown> | null | undefined): RequiredCheck[] {
  const raw = policy?.required_checks
  if (!Array.isArray(raw)) return []
  return raw.flatMap((entry): RequiredCheck[] => {
    if (typeof entry !== 'object' || entry === null) return []
    const item = entry as Record<string, unknown>
    if (typeof item.check_key !== 'string') return []
    return [
      {
        check_key: item.check_key,
        definition_digest: typeof item.definition_digest === 'string' ? item.definition_digest : '',
        required_outcome: typeof item.required_outcome === 'string' ? item.required_outcome : 'passed',
      },
    ]
  })
}
