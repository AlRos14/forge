import { useState, type ReactNode } from 'react'
import { CaretDown, CaretRight } from '@phosphor-icons/react'
import { cn } from '@/lib/cn'

export function CollapsibleSection({
  title,
  children,
  defaultOpen = false,
  badge,
  className,
  triggerClassName,
  contentClassName,
}: {
  title: ReactNode
  children: ReactNode
  defaultOpen?: boolean
  badge?: ReactNode
  className?: string
  triggerClassName?: string
  contentClassName?: string
}) {
  const [open, setOpen] = useState(defaultOpen)

  return (
    <section className={cn('space-y-3', className)}>
      <button
        type="button"
        aria-expanded={open}
        className={cn(
          'flex cursor-pointer items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground hover:text-foreground',
          triggerClassName,
        )}
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <CaretDown size={12} /> : <CaretRight size={12} />}
        {title}
        {badge ? <span className="ml-1 rounded-full bg-muted px-1.5 py-0.5 text-micro font-medium">{badge}</span> : null}
      </button>
      {open && <div className={contentClassName}>{children}</div>}
    </section>
  )
}
