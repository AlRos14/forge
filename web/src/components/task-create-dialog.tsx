import { useState, type FormEvent } from 'react'
import { useCreateTask } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { MarkdownEditor } from '@/components/ui/markdown-editor'
import { toastApiError } from '@/lib/api-error'
import { splitTaskContent } from '@/lib/task-content'
import type { Task } from '@/types/generated'

interface TaskCreateDialogProps {
  open: boolean
  projectId: string
  onOpenChange: (open: boolean) => void
  onCreated?: (task: Task) => void
}

export function TaskCreateDialog({
  open,
  projectId,
  onOpenChange,
  onCreated,
}: TaskCreateDialogProps) {
  const [content, setContent] = useState('')
  const createTask = useCreateTask(projectId)

  const reset = () => {
    setContent('')
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const taskContent = splitTaskContent(content)
    if (!content.trim()) return

    createTask.mutate(
      {
        ...taskContent,
        priority: 0,
      },
      {
        onError: (error) => toastApiError(error, 'Task creation failed'),
        onSuccess: (task) => {
          onCreated?.(task)
          reset()
          onOpenChange(false)
        },
      },
    )
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <form className="space-y-4" onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>Create Task</DialogTitle>
          </DialogHeader>
          <div className="space-y-2">
            <Label>Task</Label>
            <MarkdownEditor
              ariaLabel="Task"
              autoFocus
              placeholder={'Describe the task… Add a first-level heading like “# Plan FirePOS” if you want to set its title.'}
              minHeight="240px"
              value={content}
              onChange={setContent}
            />
            <p className="text-xs text-muted-foreground">
              A first-level “# heading” becomes the title. Without one, the first line becomes the
              title. The remaining text is saved as the Markdown description.
            </p>
          </div>
          <DialogFooter>
            <Button
              disabled={createTask.isPending}
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button disabled={createTask.isPending || !content.trim()} type="submit">
              {createTask.isPending ? 'Creating...' : 'Create Task'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
