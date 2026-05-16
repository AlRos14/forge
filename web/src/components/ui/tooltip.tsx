import { useState, type ReactNode } from 'react'
import { cn } from '@/lib/cn'

interface TooltipProps {
  content: ReactNode
  children: ReactNode
  className?: string
  side?: 'top' | 'bottom'
}

export function Tooltip({ content, children, className, side = 'top' }: TooltipProps) {
  const [show, setShow] = useState(false)

  return (
    <div
      className="relative inline-flex"
      onMouseEnter={() => setShow(true)}
      onMouseLeave={() => setShow(false)}
    >
      {children}
      {show && (
        <div
          className={cn(
            'absolute z-50 max-w-xs rounded-md border bg-popover px-3 py-1.5 text-sm text-popover-foreground shadow-md',
            side === 'top' ? 'bottom-full left-1/2 mb-2 -translate-x-1/2' : 'top-full left-1/2 mt-2 -translate-x-1/2',
            className,
          )}
        >
          {content}
        </div>
      )}
    </div>
  )
}
