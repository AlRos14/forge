import { useEffect, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/cn'

interface SheetProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  children: ReactNode
}

export function Sheet({ open, onOpenChange, children }: SheetProps) {
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onOpenChange(false)
    }
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
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0 bg-black/50" onClick={() => onOpenChange(false)} />
      {children}
    </div>,
    document.body,
  )
}

export function SheetContent({
  className,
  children,
  side = 'right',
}: {
  className?: string
  children: ReactNode
  side?: 'right' | 'left'
}) {
  const sideClasses =
    side === 'right'
      ? 'right-0 top-0 h-full border-l'
      : 'left-0 top-0 h-full border-r'
  return (
    <div
      className={cn(
        'absolute z-10 flex w-full max-w-xl flex-col bg-background text-foreground shadow-float',
        sideClasses,
        className,
      )}
      onClick={(e) => e.stopPropagation()}
    >
      {children}
    </div>
  )
}

export function SheetHeader({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  return (
    <div className={cn('flex flex-col gap-1 border-b px-5 py-4', className)}>{children}</div>
  )
}

export function SheetTitle({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  return (
    <h2 className={cn('text-base font-semibold leading-none tracking-tight', className)}>
      {children}
    </h2>
  )
}

export function SheetDescription({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  return <p className={cn('text-sm text-muted-foreground', className)}>{children}</p>
}

export function SheetBody({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  return (
    <div className={cn('flex-1 overflow-y-auto px-5 py-4', className)}>{children}</div>
  )
}

export function SheetFooter({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  return (
    <div className={cn('flex items-center justify-end gap-2 border-t px-5 py-3', className)}>
      {children}
    </div>
  )
}
