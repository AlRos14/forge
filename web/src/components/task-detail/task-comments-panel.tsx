import { GearSix } from '@phosphor-icons/react'
import { toast } from 'sonner'
import ReactMarkdown from 'react-markdown'
import rehypeSanitize from 'rehype-sanitize'
import remarkGfm from 'remark-gfm'
import { useCreateComment, useDeleteComment } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Avatar } from '@/components/ui/avatar'
import { getApiErrorMessage } from '@/lib/api-error'
import { cn } from '@/lib/cn'
import type { Dispatch, SetStateAction } from 'react'
import type { Comment, Task } from '@/types/generated'

interface TaskCommentsPanelProps {
  task: Task
  comments: Comment[]
  commentDraft: string
  setCommentDraft: Dispatch<SetStateAction<string>>
  createComment: ReturnType<typeof useCreateComment>
  deleteComment: ReturnType<typeof useDeleteComment>
  formatDate: (value?: string | null) => string
  onPostComment: () => void
}

export function TaskCommentsPanel({
  task,
  comments,
  commentDraft,
  setCommentDraft,
  createComment,
  deleteComment,
  formatDate,
  onPostComment,
}: TaskCommentsPanelProps) {
  return (
    <>
      <div className="space-y-2 rounded-lg border p-4">
        {comments.length > 0 ? (
          comments.map((comment) => (
            <div key={comment.id} className="rounded-md border p-3">
              <div className="flex items-center justify-between gap-2">
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  {comment.author_type === 'system' ? (
                    <GearSix size={12} />
                  ) : (
                    <Avatar
                      name={comment.author_name}
                      seed={comment.author_id ?? comment.author_name}
                      size="xs"
                    />
                  )}
                  <span className="font-medium text-foreground">{comment.author_name}</span>{' '}
                  <span>· {formatDate(comment.created_at)}</span>
                </div>
                {comment.author_type === 'user' ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-7 px-2 text-xs"
                    disabled={deleteComment.isPending}
                    onClick={() =>
                      deleteComment.mutate(
                        { taskId: task.id, commentId: comment.id },
                        {
                          onError: (error) =>
                            toast.error(getApiErrorMessage(error, 'Delete failed')),
                        },
                      )
                    }
                  >
                    Delete
                  </Button>
                ) : null}
              </div>
              <div
                className={cn(
                  'mt-2 prose prose-sm max-w-none dark:prose-invert',
                  comment.author_type === 'system' && 'text-muted-foreground',
                )}
              >
                <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]}>
                  {comment.content}
                </ReactMarkdown>
              </div>
            </div>
          ))
        ) : (
          <p className="text-sm text-muted-foreground">No comments yet.</p>
        )}
      </div>
      <div className="space-y-2 rounded-lg border p-4">
        <Textarea
          placeholder="Add a comment"
          value={commentDraft}
          onChange={(event) => setCommentDraft(event.target.value)}
        />
        <div className="flex justify-end">
          <Button
            size="sm"
            disabled={createComment.isPending || !commentDraft.trim()}
            onClick={onPostComment}
          >
            Post
          </Button>
        </div>
      </div>
    </>
  )
}
