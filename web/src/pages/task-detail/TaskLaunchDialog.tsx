import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import {
  ExecutionConfigBar,
  type ExecutionConfigValue,
} from '@/components/execution-config/ExecutionConfigBar'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { productTerm } from '@/lib/i18n'

interface TaskLaunchDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  isPending: boolean
  onSubmit: (config: ExecutionConfigValue, summary: string) => void
}

export function TaskLaunchDialog({
  open,
  onOpenChange,
  isPending,
  onSubmit,
}: TaskLaunchDialogProps) {
  const [launchSummary, setLaunchSummary] = useState('')
  const [launchConfig, setLaunchConfig] = useState<ExecutionConfigValue | null>(null)

  const handleSubmit = () => {
    if (!launchConfig?.agentId) return
    onSubmit(launchConfig, launchSummary)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Launch {productTerm('run')}</DialogTitle>
        </DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1">
            <Label htmlFor="launch-summary">Summary</Label>
            <Textarea
              id="launch-summary"
              placeholder="Describe what the agent should do..."
              value={launchSummary}
              onChange={(event) => setLaunchSummary(event.target.value)}
            />
          </div>
          <ExecutionConfigBar disabled={isPending} onChange={setLaunchConfig} />
          <p className="text-[11px] text-muted-foreground">
            Persistent model changes live in{' '}
            <Link to="/agents" className="underline-offset-2 hover:underline">
              Agent Settings
            </Link>
            .
          </p>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!launchConfig?.agentId || isPending} onClick={handleSubmit}>
            {isPending ? 'Launching...' : 'Launch'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
