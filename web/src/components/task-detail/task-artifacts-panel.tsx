import { useState } from 'react'
import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { apiFetch } from '@/api/client'
import { qk } from '@/api/query-keys'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { MarkdownView } from '@/components/ui/markdown-editor'
import { getApiErrorMessage } from '@/lib/api-error'
import type { ArtifactResponse, PaginatedResponse } from '@/types/generated'

export function TaskArtifactsPanel({
  taskId,
}: {
  taskId: string
}) {
  const [expandedArtifactId, setExpandedArtifactId] = useState<string | null>(null)
  const artifactsQuery = useInfiniteQuery({
    queryKey: qk.taskArtifacts(taskId),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      apiFetch<PaginatedResponse<ArtifactResponse>>(`/tasks/${taskId}/artifacts`, {
        search: { limit: 100, cursor: pageParam },
      }),
    getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
  })
  const detailQuery = useQuery({
    queryKey: qk.artifact(expandedArtifactId ?? ''),
    enabled: Boolean(expandedArtifactId),
    queryFn: () => apiFetch<ArtifactResponse>(`/artifacts/${expandedArtifactId}`),
  })
  const artifacts = artifactsQuery.data?.pages.flatMap((page) => page.items) ?? []
  const expandedArtifact = artifacts.find((artifact) => artifact.id === expandedArtifactId)

  return (
    <section className="space-y-3 rounded-lg border p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h2 className="font-semibold">Artifacts</h2>
          <p className="text-xs text-muted-foreground">
            Purpose Plan saves its completed output as a Plan Artifact. Review it here and pass its
            exact ID into a later Execution.
          </p>
        </div>
        <Button size="sm" variant="outline" onClick={() => void artifactsQuery.refetch()}>
          Refresh
        </Button>
      </div>

      {artifactsQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading Artifacts…</p> : null}
      {artifactsQuery.error ? (
        <p role="alert" className="text-sm text-destructive">
          {getApiErrorMessage(artifactsQuery.error, 'Artifacts could not be loaded')}
        </p>
      ) : null}

      <div className="space-y-2">
        {artifacts.map((artifact) => (
          <article key={artifact.id} className="space-y-2 rounded-md border p-3">
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="outline">{artifact.kind}</Badge>
              <span className="text-xs text-muted-foreground">
                {new Date(artifact.created_at).toLocaleString()}
              </span>
              <code className="min-w-0 break-all text-xs">{artifact.id}</code>
              {artifact.producer.kind === 'execution' ? (
                <span className="text-xs text-muted-foreground">
                  Execution {artifact.producer.execution_id}
                </span>
              ) : null}
              <div className="ml-auto flex flex-wrap gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setExpandedArtifactId((current) =>
                      current === artifact.id ? null : artifact.id,
                    )
                  }
                >
                  {expandedArtifactId === artifact.id ? 'Hide content' : 'View content'}
                </Button>
              </div>
            </div>

            {expandedArtifactId === artifact.id ? (
              <div className="max-h-[60vh] overflow-auto rounded bg-muted/30 p-3">
                {detailQuery.isLoading ? (
                  <p className="text-sm text-muted-foreground">Loading Artifact content…</p>
                ) : detailQuery.error ? (
                  <p role="alert" className="text-sm text-destructive">
                    {getApiErrorMessage(detailQuery.error, 'Artifact content could not be loaded')}
                  </p>
                ) : detailQuery.data?.content ? (
                  <MarkdownView content={detailQuery.data.content} className="text-sm" />
                ) : (
                  <p className="text-sm text-muted-foreground">
                    This Artifact does not contain inline text.
                  </p>
                )}
                {expandedArtifact?.metadata ? (
                  <details className="mt-3 border-t pt-2 text-xs">
                    <summary className="cursor-pointer">Metadata</summary>
                    <pre className="mt-2 overflow-auto whitespace-pre-wrap break-all">
                      {JSON.stringify(expandedArtifact.metadata, null, 2)}
                    </pre>
                  </details>
                ) : null}
              </div>
            ) : null}
          </article>
        ))}
      </div>

      {!artifactsQuery.isLoading && !artifactsQuery.error && artifacts.length === 0 ? (
        <p className="text-sm text-muted-foreground">No Artifacts have been recorded for this Task yet.</p>
      ) : null}

      {artifactsQuery.hasNextPage ? (
        <Button
          size="sm"
          variant="outline"
          disabled={artifactsQuery.isFetchingNextPage}
          onClick={() => void artifactsQuery.fetchNextPage()}
        >
          {artifactsQuery.isFetchingNextPage ? 'Loading…' : 'Load more'}
        </Button>
      ) : null}
    </section>
  )
}
