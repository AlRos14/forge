import { WarningCircle } from '@phosphor-icons/react'
import { MarkdownView } from '@/components/ui/markdown-editor'
import { cn } from '@/lib/cn'
import type { PlanArtifactDetail } from '@/types/generated'

export function PlanDocument({
  artifact,
  className,
}: {
  artifact: PlanArtifactDetail
  className?: string
}) {
  const metadata = [
    artifact.revision != null ? `Revision ${artifact.revision}` : null,
    artifact.checkpoint,
    artifact.content_digest?.slice(0, 12),
  ].filter(Boolean)

  return (
    <section
      aria-label="Task plan"
      className={cn('overflow-hidden rounded-lg border bg-card', className)}
    >
      <div className="border-b bg-muted/20 px-4 py-3">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Plan</p>
        {metadata.length > 0 ? (
          <p className="mt-1 font-mono text-micro text-muted-foreground">{metadata.join(' · ')}</p>
        ) : null}
      </div>

      {artifact.warnings.length > 0 ? (
        <div className="space-y-1 border-b px-4 py-3">
          {artifact.warnings.map((warning, index) => (
            <div
              key={`${warning}-${index}`}
              className="flex items-start gap-2 rounded-md border border-amber-500/20 bg-amber-500/10 px-2.5 py-1.5 text-xs text-amber-700 dark:text-amber-300"
            >
              <WarningCircle size={14} className="mt-0.5 shrink-0" />
              <span className="min-w-0 break-words">{warning}</span>
            </div>
          ))}
        </div>
      ) : null}

      <MarkdownView content={artifact.markdown} className="p-4" />
    </section>
  )
}
