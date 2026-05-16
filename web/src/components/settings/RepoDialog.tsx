import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { RepoForm, type RepoFormState } from '@/components/settings/RepoForm'
import type { Daemon } from '@/types/generated/api'

export function RepoDialog({
  form,
  open,
  pending,
  title,
  daemons,
  daemonId,
  onOpenChange,
  onDaemonChange,
  onSubmit,
  onUpdate,
}: {
  form: RepoFormState
  open: boolean
  pending: boolean
  title: string
  daemons: Daemon[]
  daemonId: string | undefined
  onOpenChange: (open: boolean) => void
  onDaemonChange: (daemonId: string | undefined) => void
  onSubmit: (form?: RepoFormState) => void
  onUpdate: (form: RepoFormState) => void
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        <RepoForm
          form={form}
          open={open}
          pending={pending}
          daemons={daemons}
          daemonId={daemonId}
          onCancel={() => onOpenChange(false)}
          onDaemonChange={onDaemonChange}
          key={open ? 'repo-form-open' : 'repo-form-closed'}
          onSubmit={onSubmit}
          onUpdate={onUpdate}
        />
      </DialogContent>
    </Dialog>
  )
}
