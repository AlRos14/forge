type ChatSkeletonProps = {
  rows?: number
}

const skeletonRows = [
  { width: '75%', height: 'h-10', border: 'border-l-purple-500/30' },
  { width: '40%', height: 'h-6', border: 'border-l-cyan-500/30' },
  { width: '90%', height: 'h-14', border: 'border-l-purple-500/30' },
  { width: '50%', height: 'h-6', border: 'border-l-cyan-500/30' },
  { width: '60%', height: 'h-8', border: 'border-l-blue-500/30' },
  { width: '85%', height: 'h-10', border: 'border-l-purple-500/30' },
]

export function ChatSkeleton({ rows = 6 }: ChatSkeletonProps) {
  return (
    <div className="space-y-3">
      {Array.from({ length: rows }, (_, index) => {
        const config = skeletonRows[index % skeletonRows.length]
        return (
          <div
            key={index}
            className={`animate-pulse rounded-lg border-l-[3px] ${config.border} bg-muted/20 p-3`}
            style={{ width: config.width }}
          >
            <div className={`${config.height} rounded bg-muted/40`} />
          </div>
        )
      })}
    </div>
  )
}
