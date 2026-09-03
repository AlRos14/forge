import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { AccountUsageDetails } from '@/components/account-usage'

describe('AccountUsageDetails', () => {
  it('renders Codex windows as labeled meters', () => {
    render(
      <AccountUsageDetails
        executorType="codex"
        usage={{
          rateLimits: {
            planType: 'plus',
            primary: { usedPercent: 19, windowDurationMins: 300, resetsAt: 1_788_301_664 },
            secondary: { usedPercent: 67, windowDurationMins: 10_080, resetsAt: 1_788_768_923 },
          },
        }}
      />,
    )

    expect(screen.getByText('Plus plan')).toBeTruthy()
    expect(screen.getByRole('progressbar', { name: '5-hour limit used' }).getAttribute('aria-valuenow')).toBe('19')
    expect(screen.getByRole('progressbar', { name: 'Weekly limit used' }).getAttribute('aria-valuenow')).toBe('67')
    expect(screen.getByText('81% left')).toBeTruthy()
  })

  it('renders structured Cursor categories and status', () => {
    render(
      <AccountUsageDetails
        executorType="cursor"
        usage={{
          plan: 'Pro',
          resets_at: 'Sep 30',
          categories: { included: 11, auto: 11, api: 0 },
          on_demand_enabled: false,
        }}
      />,
    )

    expect(screen.getByText('Pro plan')).toBeTruthy()
    expect(screen.getByText(/Resets Sep 30/)).toBeTruthy()
    expect(screen.getByText(/On-demand disabled/)).toBeTruthy()
    expect(screen.getByRole('progressbar', { name: 'Included used' }).getAttribute('aria-valuenow')).toBe('11')
    expect(screen.getByRole('progressbar', { name: 'API used' }).getAttribute('aria-valuenow')).toBe('0')
  })

  it('renders Codex windows from an unwrapped provider-event snapshot', () => {
    render(
      <AccountUsageDetails
        executorType="codex"
        usage={{
          planType: 'plus',
          primary: { usedPercent: 19, windowDurationMins: 300, resetsAt: 1_788_301_664 },
        }}
      />,
    )

    expect(screen.getByRole('progressbar', { name: '5-hour limit used' }).getAttribute('aria-valuenow')).toBe('19')
  })

  it('keeps older Cursor snapshots readable', () => {
    render(
      <AccountUsageDetails
        executorType="cursor"
        usage={{ pools: ['Included 25% used', 'Auto 10% used', 'API 0% used'] }}
      />,
    )

    expect(screen.getByRole('progressbar', { name: 'Included used' }).getAttribute('aria-valuenow')).toBe('25')
  })
})
