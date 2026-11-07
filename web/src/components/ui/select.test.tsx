import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { Dialog, DialogDescription, DialogTitle } from './dialog'
import { Select } from './select'

function DialogWithSelect() {
  const [open, setOpen] = useState(true)

  return (
    <>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogTitle>Choose a role</DialogTitle>
        <DialogDescription>Select the next role.</DialogDescription>
        <Select
          value="reviewer"
          options={[
            { value: 'reviewer', label: 'Reviewer' },
            { value: 'implementer', label: 'Implementer' },
          ]}
          onChange={() => {}}
          aria-label="Role"
        />
      </Dialog>
      <output>{open ? 'dialog-open' : 'dialog-closed'}</output>
    </>
  )
}

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

  it('Escape from a portalled option closes only the Select inside a Dialog', async () => {
    render(<DialogWithSelect />)

    const dialog = screen.getByRole('dialog', { name: 'Choose a role' })
    const trigger = screen.getByRole('button', { name: 'Role' })
    fireEvent.click(trigger)
    const option = await screen.findByRole('option', { name: 'Reviewer' })
    fireEvent.keyDown(option, { key: 'Escape' })

    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull())
    expect(screen.getByRole('dialog', { name: 'Choose a role' })).toBe(dialog)
    expect(screen.getByText('dialog-open').textContent).toBe('dialog-open')
    await waitFor(() => expect(document.activeElement).toBe(trigger))
  })

  it('Escape from the Select trigger closes the dropdown and keeps its Dialog open', async () => {
    render(<DialogWithSelect />)

    const dialog = screen.getByRole('dialog', { name: 'Choose a role' })
    const trigger = screen.getByRole('button', { name: 'Role' })
    fireEvent.click(trigger)
    await screen.findByRole('listbox')
    fireEvent.keyDown(trigger, { key: 'Escape' })

    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull())
    expect(screen.getByRole('dialog', { name: 'Choose a role' })).toBe(dialog)
    expect(screen.getByText('dialog-open').textContent).toBe('dialog-open')
  })
})
