import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { Select } from './select'

describe('Select', () => {
  it('moves focus through options and restores it to the trigger after selection', async () => {
    const onChange = vi.fn()
    render(
      <Select
        id="responder"
        value=""
        options={[
          { value: 'agent-1', label: 'Agent one' },
          { value: 'agent-2', label: 'Agent two' },
        ]}
        placeholder="Select responder"
        onChange={onChange}
      />,
    )

    const trigger = screen.getByRole('button', { name: 'Select responder' })
    fireEvent.keyDown(trigger, { key: 'ArrowDown' })

    const firstOption = await screen.findByRole('option', { name: 'Agent one' })
    await waitFor(() => expect(document.activeElement).toBe(firstOption))

    const secondOption = screen.getByRole('option', { name: 'Agent two' })
    fireEvent.keyDown(firstOption, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(secondOption)

    fireEvent.keyDown(secondOption, { key: 'Enter' })
    expect(onChange).toHaveBeenCalledWith('agent-2')
    await waitFor(() => expect(document.activeElement).toBe(trigger))
  })
})
