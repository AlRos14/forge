import { ChatCircle, Terminal } from '@phosphor-icons/react'
import { cn } from '@/lib/cn'
import { ChatLogViewer } from '@/components/conversation-viewer'
import { RawLogViewer } from '@/components/raw-log-viewer'
import type { LogEntry } from '@/types/generated'

export type ExecutionViewerMode = 'chat' | 'raw'

type ExecutionViewerProps = {
  logs: LogEntry[]
  rawLogs?: LogEntry[]
  mode: ExecutionViewerMode
  userPrompt?: string | null
  errorMessage?: string | null
  autoScroll?: boolean
  className?: string
  isLoadingHistory?: boolean
  onModeChange: (mode: ExecutionViewerMode) => void
  onNewLog?: (log: LogEntry) => void
  onLoadEarlier?: () => void
  onScrollBottomChange?: (isAtBottom: boolean) => void
}

export function ExecutionViewer({
  logs,
  rawLogs,
  mode,
  userPrompt,
  errorMessage,
  autoScroll,
  className,
  isLoadingHistory,
  onModeChange,
  onNewLog,
  onLoadEarlier,
  onScrollBottomChange,
}: ExecutionViewerProps) {
  const visibleLogCount = mode === 'raw' ? (rawLogs ?? logs).length : logs.length

  return (
    <div className={cn('flex h-full flex-col', className)}>
      {/* Segmented Control */}
      <div className="flex items-center border-b px-3 py-1.5">
        <div className="inline-flex items-center rounded-md bg-muted p-0.5">
          <button
            type="button"
            onClick={() => onModeChange('chat')}
            className={cn(
              'inline-flex items-center gap-1.5 rounded-[5px] px-2.5 py-1 text-xs font-medium transition-all cursor-pointer',
              mode === 'chat'
                ? 'bg-background text-foreground shadow-xs'
                : 'text-muted-foreground hover:text-foreground',
            )}
          >
            <ChatCircle className="h-3.5 w-3.5" weight={mode === 'chat' ? 'fill' : 'regular'} />
            Chat
          </button>
          <button
            type="button"
            onClick={() => onModeChange('raw')}
            className={cn(
              'inline-flex items-center gap-1.5 rounded-[5px] px-2.5 py-1 text-xs font-medium transition-all cursor-pointer',
              mode === 'raw'
                ? 'bg-background text-foreground shadow-xs'
                : 'text-muted-foreground hover:text-foreground',
            )}
          >
            <Terminal className="h-3.5 w-3.5" weight={mode === 'raw' ? 'fill' : 'regular'} />
            Raw
          </button>
        </div>
        <span className="ml-auto text-micro text-muted-foreground/50 tabular-nums">
          {visibleLogCount > 0 && `${visibleLogCount} entries`}
        </span>
      </div>
      <div className="relative min-h-0 flex-1">
        <div className="absolute inset-0">
          {mode === 'chat' ? (
            <ChatLogViewer
              logs={logs}
              userPrompt={userPrompt}
              errorMessage={errorMessage}
              autoScroll={autoScroll}
              isLoadingHistory={isLoadingHistory}
              onNewLog={onNewLog}
              onLoadEarlier={onLoadEarlier}
              onScrollBottomChange={onScrollBottomChange}
              className="h-full"
            />
          ) : (
            <RawLogViewer
              logs={rawLogs ?? logs}
              autoScroll={autoScroll}
              onLoadEarlier={onLoadEarlier}
              onScrollBottomChange={onScrollBottomChange}
              className="h-full"
            />
          )}
        </div>
      </div>
    </div>
  )
}
