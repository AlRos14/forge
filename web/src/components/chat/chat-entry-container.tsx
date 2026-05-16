import { CaretDown } from '@phosphor-icons/react'
import { useState, type KeyboardEvent, type ReactNode } from 'react'
import type { ChatEntryStatus } from '@/components/chat/types'
import { cn } from '@/lib/cn'

export type ChatEntryContainerVariant =
  | 'assistant'
  | 'user'
  | 'system'
  | 'tool'
  | 'error'
  | 'approval'
  | 'session'
  | 'unknown'

type ChatEntryContainerProps = {
  variant: ChatEntryContainerVariant
  status?: ChatEntryStatus
  icon?: ReactNode
  header: ReactNode
  children?: ReactNode
  defaultCollapsed?: boolean
  onToggle?: (open: boolean) => void
}

const variantClasses: Record<ChatEntryContainerVariant, string> = {
  assistant: 'border-l-purple-500/70 bg-purple-500/[0.03] hover:bg-purple-500/[0.06]',
  user: 'border-l-blue-500/70 bg-blue-500/[0.03] hover:bg-blue-500/[0.06]',
  system: 'border-l-gray-400/50 bg-muted/20 hover:bg-muted/30',
  tool: 'border-l-cyan-500/60 bg-cyan-500/[0.02] hover:bg-cyan-500/[0.04]',
  error: 'border-l-red-500/70 bg-red-500/[0.04] hover:bg-red-500/[0.07]',
  approval: 'border-l-amber-500/70 bg-amber-500/[0.03] hover:bg-amber-500/[0.06]',
  session: 'border-l-gray-300/40 bg-muted/10 hover:bg-muted/20',
  unknown: 'border-l-gray-300/40 border-dashed bg-muted/10 hover:bg-muted/20',
}

const statusClasses: Record<ChatEntryStatus, string> = {
  pending: 'bg-gray-400',
  success: 'bg-emerald-500',
  failed: 'bg-red-500',
  denied: 'bg-red-500',
  timed_out: 'bg-orange-500',
  pending_approval: 'bg-amber-500',
}

export function ChatEntryContainer({
  variant,
  status,
  icon,
  header,
  children,
  defaultCollapsed = false,
  onToggle,
}: ChatEntryContainerProps) {
  const hasChildren = children !== undefined && children !== null
  const [open, setOpen] = useState(!defaultCollapsed)

  const toggleOpen = () => {
    if (!hasChildren) {
      return
    }

    const nextOpen = !open
    setOpen(nextOpen)
    onToggle?.(nextOpen)
  }

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Enter' && event.key !== ' ') {
      return
    }

    event.preventDefault()
    toggleOpen()
  }

  const headerContent = (
    <>
      {icon ? <span className="shrink-0">{icon}</span> : null}
      {status ? (
        <span className="relative flex h-2 w-2 shrink-0" aria-hidden="true">
          {status === 'pending' ? (
            <span className={cn('absolute inline-flex h-2 w-2 animate-ping rounded-full', statusClasses[status])} />
          ) : null}
          <span className={cn('relative inline-flex h-2 w-2 rounded-full', statusClasses[status])} />
        </span>
      ) : null}
      <span className="min-w-0 flex-1">{header}</span>
      {hasChildren ? (
        <CaretDown className={cn('h-3.5 w-3.5 shrink-0 text-muted-foreground/50 transition-transform', open ? 'rotate-0' : '-rotate-90')} />
      ) : null}
    </>
  )

  return (
    <div
      className={cn('rounded-lg border-l-[3px] transition-colors', variantClasses[variant])}
      data-chat-entry-status={status}
      data-chat-entry-variant={variant}
      data-scroll-anchor-target=""
    >
      {hasChildren ? (
        <div
          className="flex items-center gap-2 px-3 py-2 text-left text-sm font-medium cursor-pointer select-none"
          role="button"
          aria-expanded={open}
          tabIndex={0}
          onClick={toggleOpen}
          onKeyDown={handleKeyDown}
        >
          {headerContent}
        </div>
      ) : (
        <div className="flex items-center gap-2 px-3 py-2 text-left text-sm font-medium">{headerContent}</div>
      )}
      {open && hasChildren ? <div className="px-3 pb-3 pt-1">{children}</div> : null}
    </div>
  )
}
