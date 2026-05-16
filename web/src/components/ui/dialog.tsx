import { useEffect, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/cn'

interface DialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  children: ReactNode
  className?: string
}

export function Dialog({ open, onOpenChange, children, className }: DialogProps) {
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onOpenChange(false) }
    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    document.addEventListener('keydown', onKey)
    return () => {
      document.body.style.overflow = prev
      document.removeEventListener('keydown', onKey)
    }
  }, [open, onOpenChange])

  if (!open) return null

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/50" onClick={() => onOpenChange(false)} />
      <div
        className={cn(
          'relative z-10 max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-lg border bg-background p-0 text-foreground shadow-lg',
          'animate-in fade-in-0 zoom-in-95',
          className,
        )}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>,
    document.body,
  )
}

export function DialogContent({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn('p-6', className)}>{children}</div>
}

export function DialogHeader({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn('flex flex-col space-y-1.5 text-center sm:text-left', className)}>{children}</div>
}

export function DialogTitle({ className, children }: { className?: string; children: ReactNode }) {
  return <h2 className={cn('text-lg font-semibold leading-none tracking-tight', className)}>{children}</h2>
}

export function DialogDescription({ className, children }: { className?: string; children: ReactNode }) {
  return <p className={cn('text-sm text-muted-foreground', className)}>{children}</p>
}

export function DialogFooter({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn('flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2', className)}>{children}</div>
}
