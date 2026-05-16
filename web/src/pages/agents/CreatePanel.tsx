import { Button } from '@/components/ui/button'
import type { Daemon } from '@/types/generated'
import { AgentForm } from './AgentForm'
import type { AgentFormState } from './form-utils'

export function CreatePanel({
  form,
  daemons,
  showDaemonSelector,
  pending,
  onUpdate,
  onSubmit,
  onCancel,
}: {
  form: AgentFormState
  daemons: Daemon[]
  showDaemonSelector: boolean
  pending: boolean
  onUpdate: (form: AgentFormState) => void
  onSubmit: () => void
  onCancel: () => void
}) {
  const disabled = pending || !form.name.trim()

  return (
    <div className="flex flex-1 flex-col overflow-y-auto">
      <header className="flex shrink-0 items-center justify-between px-6 py-4">
        <h2 className="text-lg font-semibold">New Agent</h2>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button size="sm" disabled={disabled} onClick={onSubmit}>
            {pending ? 'Creating...' : 'Create'}
          </Button>
        </div>
      </header>
      <div className="flex-1 overflow-y-auto px-8 py-6">
        <AgentForm
          form={form}
          mode="create"
          daemons={daemons}
          showDaemonSelector={showDaemonSelector}
          onChange={onUpdate}
        />
      </div>
    </div>
  )
}
