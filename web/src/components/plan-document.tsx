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
  return (
    <section
      aria-label="Task plan"
      className={cn('overflow-hidden rounded-lg border bg-card', className)}
    >
      <div className="border-b bg-muted/20 px-4 py-3">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Plan</p>
        <dl className="mt-2 grid gap-x-5 gap-y-1 text-micro text-muted-foreground sm:grid-cols-2">
          <div className="min-w-0">
            <dt className="inline">Artifact </dt>
            <dd className="inline break-all font-mono" title={artifact.artifact_id}>
              {artifact.artifact_id}
            </dd>
          </div>
          <div className="min-w-0">
            <dt className="inline">Producer </dt>
            <dd className="inline break-all font-mono">
              {artifact.producer.kind}:{artifact.producer.id}
            </dd>
          </div>
          <div className="min-w-0">
            <dt className="inline">Execution </dt>
            <dd className="inline break-all font-mono" title={artifact.producer_execution_id}>
              {artifact.producer_execution_id}
            </dd>
          </div>
          <div className="min-w-0">
            <dt className="inline">Created </dt>
            <dd className="inline">{artifact.created_at}</dd>
          </div>
          {artifact.content_digest ? (
            <div className="min-w-0 sm:col-span-2">
              <dt className="inline">SHA-256 </dt>
              <dd className="inline break-all font-mono">{artifact.content_digest}</dd>
            </div>
          ) : null}
        </dl>
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
