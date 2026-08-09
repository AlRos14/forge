import { Robot } from '@phosphor-icons/react'
import { productTerm } from '@/lib/i18n'

type ChatEmptyStateProps = {
  title?: string
  description?: string
}

export function ChatEmptyState({
  title = 'Waiting for activity',
  description = `${productTerm('run', 0)} logs will appear here once the agent starts working.`,
}: ChatEmptyStateProps) {
  return (
    <div className="flex min-h-48 flex-col items-center justify-center px-6 py-16 text-center">
      <div className="mb-4 rounded-xl bg-muted/50 p-3">
        <Robot className="h-8 w-8 text-muted-foreground/40" weight="duotone" />
      </div>
      <h3 className="text-sm font-medium text-foreground">{title}</h3>
      <p className="mt-1.5 max-w-xs text-xs text-muted-foreground leading-relaxed">{description}</p>
    </div>
  )
}
