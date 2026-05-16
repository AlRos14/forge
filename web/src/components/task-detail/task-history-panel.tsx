import { TransitionTimeline } from '@/components/transition-timeline'

export function TaskHistoryPanel({ taskId }: { taskId: string }) {
  return (
    <div className="space-y-4">
      <TransitionTimeline taskId={taskId} />
    </div>
  )
}
