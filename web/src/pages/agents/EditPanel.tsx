import { Check, Trash, X } from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import type { Agent, Daemon } from '@/types/generated'
import { AgentForm } from './AgentForm'
import { formatDate, type AgentFormState } from './form-utils'
import { AgentSection } from './shared'

export function EditPanel({
  agent,
  form,
  daemons,
  showDaemonSelector,
  pending,
  canDelete,
  deletePending,
  confirmingDelete,
  onUpdate,
  onSubmit,
  onCancel,
  onDelete,
  onConfirmDelete,
  onCancelDelete,
}: {
  agent: Agent
  form: AgentFormState
  daemons: Daemon[]
  showDaemonSelector: boolean
  pending: boolean
  canDelete: boolean
  deletePending: boolean
  confirmingDelete: boolean
  onUpdate: (form: AgentFormState) => void
  onSubmit: () => void
  onCancel: () => void
  onDelete: () => void
  onConfirmDelete: () => void
  onCancelDelete: () => void
}) {
  const disabled = pending || !form.name.trim()

  return (
    <div className="flex flex-1 flex-col overflow-y-auto">
      <header className="flex shrink-0 items-center justify-between border-b px-6 py-4">
        <div>
          <h2 className="text-lg font-semibold">Edit Agent</h2>
          <p className="mt-0.5 text-xs text-muted-foreground">
            v{agent.version} · Updated {formatDate(agent.updated_at)}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button size="sm" disabled={disabled} onClick={onSubmit}>
            {pending ? 'Saving...' : 'Save'}
          </Button>
        </div>
      </header>
      <div className="flex-1 overflow-y-auto px-8 py-6">
        <AgentForm
          form={form}
          mode="edit"
          daemons={daemons}
          showDaemonSelector={showDaemonSelector}
          onChange={onUpdate}
          agentId={agent.id}
        />

        <AgentSection title="Danger zone" danger>
          {confirmingDelete ? (
            <div className="flex items-center gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-3">
              <p className="flex-1 text-sm">
                Delete <strong>{agent.name}</strong>? History will be preserved.
              </p>
              <Button size="sm" variant="outline" onClick={onCancelDelete}>
                <X size={14} />
              </Button>
              <Button size="sm" variant="destructive" disabled={deletePending || !canDelete} onClick={onDelete}>
                <Check size={14} className="mr-1" />
                {deletePending ? 'Deleting...' : 'Confirm'}
              </Button>
            </div>
          ) : (
            <Button
              size="sm"
              variant="outline"
              className="gap-1.5 text-destructive hover:bg-destructive/10 hover:text-destructive"
              disabled={!canDelete}
              onClick={onConfirmDelete}
            >
              <Trash size={14} />
              {canDelete ? 'Delete agent' : 'Has active tasks'}
            </Button>
          )}
        </AgentSection>
      </div>
    </div>
  )
}
