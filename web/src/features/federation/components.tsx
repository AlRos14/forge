import {
  ArrowClockwise,
  CheckCircle,
  CircleNotch,
  Info,
  WarningCircle,
} from '@phosphor-icons/react'
import type { ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { cn } from '@/lib/cn'

export function StatusDot({ status, className }: { status: string | null | undefined; className?: string }) {
  const normalized = status?.toLowerCase() ?? 'unknown'
  const tone =
    normalized.includes('ready') ||
    normalized.includes('healthy') ||
    normalized.includes('complete') ||
    normalized.includes('active') ||
    normalized === 'idle'
      ? 'bg-success'
      : normalized.includes('error') ||
          normalized.includes('failed') ||
          normalized.includes('offline') ||
          normalized.includes('unavailable')
        ? 'bg-destructive'
        : normalized.includes('attention') ||
            normalized.includes('degraded') ||
            normalized.includes('paused') ||
            normalized.includes('stalled') ||
            normalized.includes('stale')
          ? 'bg-warning'
          : 'bg-muted-foreground'

  return <span aria-hidden className={cn('h-2 w-2 shrink-0 rounded-full', tone, className)} />
}

export function StateBadge({ status, label }: { status: string | null | undefined; label?: string }) {
  const normalized = status?.toLowerCase() ?? 'unknown'
  const tone =
    normalized.includes('ready') ||
    normalized.includes('healthy') ||
    normalized.includes('complete') ||
    normalized.includes('active') ||
    normalized === 'idle'
      ? 'border-success/30 bg-success/10 text-success'
      : normalized.includes('error') ||
          normalized.includes('failed') ||
          normalized.includes('offline') ||
          normalized.includes('unavailable')
        ? 'border-destructive/30 bg-destructive/10 text-destructive'
        : normalized.includes('attention') ||
            normalized.includes('degraded') ||
            normalized.includes('paused') ||
            normalized.includes('stalled') ||
            normalized.includes('stale')
          ? 'border-warning/40 bg-warning/10 text-warning'
          : 'border-border bg-muted text-muted-foreground'

  return (
    <span className={cn('inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 font-mono text-micro font-semibold uppercase tracking-[0.7px]', tone)}>
      <StatusDot status={status} />
      {label ?? status ?? 'Unknown'}
    </span>
  )
}

export function SectionKicker({ children }: { children: ReactNode }) {
  return <p className="font-mono text-micro font-semibold uppercase tracking-[1px] text-muted-foreground">{children}</p>
}

export function PageHeader({
  eyebrow,
  title,
  description,
  actions,
}: {
  eyebrow: string
  title: string
  description?: string
  actions?: ReactNode
}) {
  return (
    <header className="flex flex-wrap items-start justify-between gap-4 border-b border-border-subtle pb-5">
      <div className="min-w-0">
        <SectionKicker>{eyebrow}</SectionKicker>
        <h1 className="mt-1 text-page font-semibold tracking-tight text-foreground">{title}</h1>
        {description ? <p className="mt-2 max-w-2xl text-sm text-muted-foreground">{description}</p> : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  )
}

export function LoadingPanel({ label = 'Loading projection' }: { label?: string }) {
  return (
    <Card className="border-border-subtle bg-card/70 p-6">
      <div className="flex items-center gap-3 text-sm text-muted-foreground" role="status" aria-live="polite">
        <CircleNotch size={18} className="animate-spin text-primary" aria-hidden />
        {label}
      </div>
      <div className="mt-5 space-y-2" aria-hidden>
        <div className="h-3 w-2/5 animate-pulse rounded bg-muted" />
        <div className="h-3 w-4/5 animate-pulse rounded bg-muted" />
        <div className="h-3 w-3/5 animate-pulse rounded bg-muted" />
      </div>
    </Card>
  )
}

export function EmptyPanel({ title, description, icon = <Info size={19} />, action }: { title: string; description: string; icon?: ReactNode; action?: ReactNode }) {
  return (
    <Card className="border-dashed border-border-subtle bg-card/50 p-8 text-center">
      <div className="mx-auto flex h-9 w-9 items-center justify-center rounded-full bg-muted text-muted-foreground">{icon}</div>
      <h2 className="mt-3 text-sm font-semibold text-foreground">{title}</h2>
      <p className="mx-auto mt-1 max-w-md text-sm leading-6 text-muted-foreground">{description}</p>
      {action ? <div className="mt-4 flex justify-center">{action}</div> : null}
    </Card>
  )
}

export function ErrorPanel({
  title = 'Could not load this view',
  description,
  onRetry,
}: {
  title?: string
  description?: string
  onRetry?: () => void
}) {
  return (
    <Card className="border-destructive/30 bg-destructive/5 p-6" role="alert">
      <div className="flex items-start gap-3">
        <WarningCircle size={20} className="mt-0.5 shrink-0 text-destructive" aria-hidden />
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold text-foreground">{title}</h2>
          <p className="mt-1 text-sm leading-6 text-muted-foreground">
            {description ?? 'The authoritative projection could not be reached. Try again when the server is available.'}
          </p>
          {onRetry ? (
            <Button variant="outline" size="sm" className="mt-4" onClick={onRetry}>
              <ArrowClockwise size={14} aria-hidden />
              Retry
            </Button>
          ) : null}
        </div>
      </div>
    </Card>
  )
}

export function HealthyIcon({ className }: { className?: string }) {
  return <CheckCircle size={16} className={cn('text-success', className)} aria-hidden />
}
