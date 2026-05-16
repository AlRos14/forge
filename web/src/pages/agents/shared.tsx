import type { ReactNode } from 'react'
import { Label } from '@/components/ui/label'
import { cn } from '@/lib/cn'

export function InfoField({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="mb-0.5 text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {label}
      </p>
      <p className="text-sm">{value}</p>
    </div>
  )
}

export function FormField({
  label,
  htmlFor,
  children,
}: {
  label: string
  htmlFor: string
  children: ReactNode
}) {
  return (
    <div className="space-y-1.5">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
    </div>
  )
}

export function AgentSection({
  title,
  description,
  children,
  danger,
}: {
  title: string
  description?: string
  children: ReactNode
  danger?: boolean
}) {
  return (
    <section className="border-b py-6 last:border-b-0">
      <div className="grid grid-cols-[200px_1fr] items-start gap-8">
        <div>
          <h3 className={cn('text-[13px] font-semibold leading-snug', danger ? 'text-red-300' : 'text-foreground')}>
            {title}
          </h3>
          {description && <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{description}</p>}
        </div>
        <div>{children}</div>
      </div>
    </section>
  )
}
