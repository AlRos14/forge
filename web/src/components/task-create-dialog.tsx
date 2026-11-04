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
import { Select } from '@/components/ui/select'
import { MarkdownEditor } from '@/components/ui/markdown-editor'
import { toastApiError } from '@/lib/api-error'
import { splitTaskContent } from '@/lib/task-content'
import type { Task, TaskType } from '@/types/generated'

const taskTypes: { value: TaskType; label: string }[] = [
  { value: 'implementation', label: 'Implementation' },
  { value: 'planning', label: 'Planning' },
  { value: 'discovery', label: 'Discovery' },
  { value: 'review', label: 'Review' },
  { value: 'validation', label: 'Validation' },
]

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
  const [taskType, setTaskType] = useState<TaskType>('implementation')
  const createTask = useCreateTask(projectId)

  const reset = () => {
    setContent('')
    setTaskType('implementation')
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const taskContent = splitTaskContent(content)
    if (!content.trim()) return

    createTask.mutate(
      {
        ...taskContent,
        task_type: taskType,
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
          <details className="rounded-md border px-3 py-2">
            <summary className="cursor-pointer text-sm font-medium">More options</summary>
            <div className="mt-3 space-y-2">
              <Label htmlFor="task-type">Task type</Label>
              <Select
                id="task-type"
                value={taskType}
                options={taskTypes.map((type) => ({ value: type.value, label: type.label }))}
                onChange={(v) => setTaskType(v as TaskType)}
              />
            </div>
          </details>
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
