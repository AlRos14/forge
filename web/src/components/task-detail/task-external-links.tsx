import { useState } from 'react'
import { ArrowSquareOut, LinkSimple, Plus, X } from '@phosphor-icons/react'
import { toast } from 'sonner'
import {
  useCreateExternalLink,
  useDeleteExternalLink,
  useExternalLinksQuery,
} from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { getApiErrorMessage } from '@/lib/api-error'

function platformLabel(platform: string): string {
  switch (platform.toLowerCase()) {
    case 'github':
      return 'GitHub'
    case 'gitea':
      return 'Gitea'
    default:
      return platform
  }
}

export function TaskExternalLinks({ taskId }: { taskId: string }) {
  const { data: links, isLoading } = useExternalLinksQuery(taskId)
  const createLink = useCreateExternalLink()
  const deleteLink = useDeleteExternalLink()

  const [showInput, setShowInput] = useState(false)
  const [issueNumber, setIssueNumber] = useState('')

  const handleCreate = () => {
    const num = Number(issueNumber.trim())
    if (!Number.isFinite(num) || num <= 0 || !Number.isInteger(num)) {
      toast.error('Enter a valid issue number')
      return
    }
    createLink.mutate(
      { taskId, remoteIssueNumber: num },
      {
        onSuccess: () => {
          setIssueNumber('')
          setShowInput(false)
          toast.success('External link created')
        },
        onError: (error) => {
          toast.error(getApiErrorMessage(error, 'Failed to create external link'))
        },
      },
    )
  }

  const handleDelete = (linkId: string) => {
    deleteLink.mutate(
      { taskId, linkId },
      {
        onError: (error) => {
          toast.error(getApiErrorMessage(error, 'Failed to remove external link'))
        },
      },
    )
  }

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      handleCreate()
    }
    if (e.key === 'Escape') {
      setShowInput(false)
      setIssueNumber('')
    }
  }

  if (isLoading) return null

  const hasLinks = links && links.length > 0

  return (
    <div>
      {hasLinks ? (
        <div className="space-y-0.5">
          {links.map((link) => (
            <div key={link.id} className="group flex items-center gap-1">
              <span className="shrink-0 text-muted-foreground">
                {platformLabel(link.platform)}
              </span>
              <a
                href={link.remote_url}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-0.5 text-primary hover:underline"
              >
                {link.remote_owner}/{link.remote_repo}#{link.remote_issue_number}
                <ArrowSquareOut size={11} className="shrink-0 opacity-60" />
              </a>
              <button
                type="button"
                className="ml-0.5 shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100"
                title="Remove link"
                disabled={deleteLink.isPending}
                onClick={() => handleDelete(link.id)}
              >
                <X size={12} />
              </button>
            </div>
          ))}
        </div>
      ) : null}

      {showInput ? (
        <div className="mt-1 flex items-center gap-1">
          <Input
            autoFocus
            type="number"
            min={1}
            step={1}
            placeholder="Issue #"
            className="h-6 w-24 text-xs"
            value={issueNumber}
            onChange={(e) => setIssueNumber(e.target.value)}
            onKeyDown={handleKeyDown}
          />
          <Button
            size="sm"
            variant="ghost"
            className="h-6 px-1.5 text-xs"
            disabled={createLink.isPending || !issueNumber.trim()}
            onClick={handleCreate}
          >
            {createLink.isPending ? 'Linking...' : 'Add'}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-6 px-1 text-xs text-muted-foreground"
            onClick={() => {
              setShowInput(false)
              setIssueNumber('')
            }}
          >
            Cancel
          </Button>
        </div>
      ) : (
        <button
          type="button"
          className="mt-0.5 inline-flex items-center gap-0.5 text-muted-foreground hover:text-foreground"
          onClick={() => setShowInput(true)}
        >
          {hasLinks ? (
            <Plus size={11} />
          ) : (
            <LinkSimple size={11} />
          )}
          <span>{hasLinks ? 'Link another issue' : 'Link issue'}</span>
        </button>
      )}
    </div>
  )
}
