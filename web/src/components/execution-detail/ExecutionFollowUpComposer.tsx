import { PaperPlaneTilt, Spinner } from '@phosphor-icons/react'

import {
  ExecutionConfigBar,
  type ExecutionConfigValue,
} from '@/components/execution-config/ExecutionConfigBar'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/cn'

export function ExecutionFollowUpComposer({
  message,
  onMessageChange,
  isPending,
  showConfigBar,
  onToggleConfigBar,
  onSend,
  onConfigChange,
  initialAgentId,
  initialOverrides,
  executorTypeConstraint,
  textareaRef,
}: {
  message: string
  onMessageChange: (value: string) => void
  isPending: boolean
  showConfigBar: boolean
  onToggleConfigBar: () => void
  onSend: () => void
  onConfigChange: (value: ExecutionConfigValue | null) => void
  initialAgentId: string | null
  initialOverrides: { modelId: string | null; reasoningEffort: string | null; permissionPolicy: string | null }
  executorTypeConstraint: string | null
  textareaRef: React.RefObject<HTMLTextAreaElement>
}) {
  return (
    <div className="border-t bg-card">
      <div className="p-3">
        <div className="rounded-lg border bg-background shadow-xs transition-shadow focus-within:shadow-soft focus-within:border-primary/30">
          <textarea
            ref={textareaRef}
            placeholder="Send a follow-up message..."
            value={message}
            rows={1}
            className="w-full resize-none bg-transparent px-3 pt-2.5 pb-1 text-sm placeholder:text-muted-foreground/60 focus:outline-none"
            disabled={isPending}
            onChange={(e) => {
              onMessageChange(e.target.value)
              const el = e.target
              el.style.height = 'auto'
              el.style.height = `${Math.min(el.scrollHeight, 120)}px`
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                e.preventDefault()
                onSend()
              }
            }}
          />
          <div className="flex items-center justify-between px-2 pb-2">
            <button
              type="button"
              className={cn(
                'text-xs text-muted-foreground/60 hover:text-muted-foreground transition-colors cursor-pointer px-1',
                showConfigBar && 'text-primary',
              )}
              onClick={onToggleConfigBar}
            >
              Configure
            </button>
            <div className="flex items-center gap-2">
              <span className="text-micro text-muted-foreground/40">
                {navigator.platform?.includes('Mac') ? '⌘' : 'Ctrl'}+Enter
              </span>
              <Button
                size="sm"
                className="h-7 gap-1.5 rounded-md px-3"
                disabled={isPending || !message.trim()}
                onClick={onSend}
              >
                {isPending ? (
                  <Spinner className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <PaperPlaneTilt className="h-3.5 w-3.5" />
                )}
                <span className="text-xs">Send</span>
              </Button>
            </div>
          </div>
        </div>
      </div>
      {showConfigBar && (
        <div className="border-t px-3 py-2 animate-slide-in">
          <ExecutionConfigBar
            initialAgentId={initialAgentId}
            initialOverrides={initialOverrides}
            executorTypeConstraint={executorTypeConstraint}
            disabled={isPending}
            useRecentSelections={false}
            showAgentSelector={true}
            showPolicySelector={false}
            onChange={onConfigChange}
          />
        </div>
      )}
    </div>
  )
}
