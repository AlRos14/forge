import type { ReactNode } from 'react'
import { cn } from '@/lib/cn'

export function SettingsSection({
  title,
  description,
  children,
  danger,
}: {
  title: ReactNode
  description?: string
  children: ReactNode
  danger?: boolean
}) {
  return (
    <section className="border-b py-6 last:border-b-0">
      <div className="grid grid-cols-[220px_1fr] items-start gap-8">
        <div>
          <h3
            className={cn(
              'text-[13px] font-semibold leading-snug',
              danger ? 'text-red-300' : 'text-foreground',
            )}
          >
            {title}
          </h3>
          {description && (
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{description}</p>
          )}
        </div>
        <div>{children}</div>
      </div>
    </section>
  )
}
