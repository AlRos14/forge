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
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { MarkdownEditor } from '@/components/ui/markdown-editor'
import { toastApiError } from '@/lib/api-error'
import type { Task, TaskType } from '@/types/generated'

const taskTypes: { value: TaskType; label: string }[] = [
  { value: 'task', label: 'Task' },
  { value: 'planning_task', label: 'Planning Task' },
  { value: 'discovery', label: 'Discovery' },
  { value: 'sub_task', label: 'Sub Task' },
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
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const [taskType, setTaskType] = useState<TaskType>('task')
  const createTask = useCreateTask(projectId)

  const reset = () => {
    setTitle('')
    setDescription('')
    setTaskType('task')
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const trimmedTitle = title.trim()
    if (!trimmedTitle) return

    createTask.mutate(
      {
        title: trimmedTitle,
        description: description.trim() || undefined,
        task_type: taskType,
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
            <Label htmlFor="task-title">Title</Label>
            <Input
              required
              autoFocus
              id="task-title"
              placeholder="What needs to be done?"
              value={title}
              onChange={(event) => setTitle(event.target.value)}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="task-type">Type</Label>
            <Select
              id="task-type"
              value={taskType}
              options={taskTypes.map((type) => ({ value: type.value, label: type.label }))}
              onChange={(v) => setTaskType(v as TaskType)}
            />
          </div>
          <div className="space-y-2">
            <Label>
              Description{' '}
              <span className="text-xs font-normal text-muted-foreground">(optional)</span>
            </Label>
            <MarkdownEditor
              placeholder="Add more details... (markdown supported)"
              minHeight="96px"
              value={description}
              onChange={setDescription}
            />
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
            <Button disabled={createTask.isPending || !title.trim()} type="submit">
              {createTask.isPending ? 'Creating...' : 'Create Task'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
