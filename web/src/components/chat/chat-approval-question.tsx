import { CheckCircle, Question, XCircle } from '@phosphor-icons/react'

import { ChatEntryContainer } from '@/components/chat/chat-entry-container'
import type { ChatApprovalEntry } from '@/components/chat/types'

type ChatApprovalQuestionProps = {
  entry: ChatApprovalEntry
}

function toContainerStatus(status: ChatApprovalEntry['status']) {
  return status === 'pending' || status === 'success' || status === 'failed' ? status : undefined
}

export function ChatApprovalQuestion({ entry }: ChatApprovalQuestionProps) {
  const isApproved = entry.decision === 'accept'
  const isDeclined = entry.decision === 'decline'

  return (
    <ChatEntryContainer
      variant="approval"
      status={toContainerStatus(entry.status)}
      icon={
        isApproved ? (
          <CheckCircle weight="duotone" />
        ) : isDeclined ? (
          <XCircle weight="duotone" />
        ) : (
          <Question weight="duotone" />
        )
      }
      header={<span className="font-semibold">{entry.question}</span>}
      defaultCollapsed={true}
    >
      <div className="space-y-2">
        {isApproved || isDeclined ? (
          <span
            className={
              isApproved
                ? 'inline-flex items-center rounded-md bg-green-500/10 px-2 py-1 text-xs font-medium text-green-700 dark:text-green-400'
                : 'inline-flex items-center rounded-md bg-red-500/10 px-2 py-1 text-xs font-medium text-red-700 dark:text-red-400'
            }
          >
            {isApproved ? 'Approved' : 'Declined'}
          </span>
        ) : null}
        {entry.rationale ? <p className="text-sm text-muted-foreground">{entry.rationale}</p> : null}
      </div>
    </ChatEntryContainer>
  )
}
