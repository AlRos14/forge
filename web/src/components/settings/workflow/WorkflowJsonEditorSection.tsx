import { lazy, Suspense } from 'react'
import type { Dispatch, SetStateAction } from 'react'
import { ArrowsOut } from '@phosphor-icons/react'
import { Light as SyntaxHighlighter } from 'react-syntax-highlighter'
import json from 'react-syntax-highlighter/dist/esm/languages/hljs/json'
import { atomOneDark, atomOneLight } from 'react-syntax-highlighter/dist/esm/styles/hljs'
import { SettingsSection } from '@/components/settings/SettingsSection'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Skeleton } from '@/components/ui/skeleton'

SyntaxHighlighter.registerLanguage('json', json)

const JsonEditor = lazy(() =>
  import('@/components/ui/code-editor').then((m) => ({ default: m.JsonEditor })),
)

export function WorkflowJsonEditorSection({
  workflowJson,
  setWorkflowJson,
  editorOpen,
  setEditorOpen,
  theme,
  isSaving,
  onSave,
}: {
  workflowJson: string
  setWorkflowJson: Dispatch<SetStateAction<string>>
  editorOpen: boolean
  setEditorOpen: Dispatch<SetStateAction<boolean>>
  theme: 'dark' | 'light'
  isSaving: boolean
  onSave: () => void
}) {
  return (
    <>
      <SettingsSection
        title="Definition (JSON)"
        description="Edit the workflow definition directly. Leave as {} to use the built-in default."
      >
        <button
          type="button"
          aria-label="Open workflow JSON editor"
          onClick={() => setEditorOpen(true)}
          className="group relative w-full cursor-pointer overflow-hidden rounded-md border border-border text-left transition-colors hover:border-primary/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        >
          <div className="pointer-events-none max-h-44 overflow-hidden">
            <SyntaxHighlighter
              language="json"
              style={theme === 'dark' ? atomOneDark : atomOneLight}
              customStyle={{
                margin: 0,
                padding: '12px 16px',
                fontSize: '11px',
                lineHeight: '1.6',
                background: 'transparent',
                fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
              }}
            >
              {workflowJson || '{}'}
            </SyntaxHighlighter>
          </div>
          <div className="absolute inset-x-0 bottom-0 flex h-20 items-end justify-end bg-gradient-to-t from-card via-card/70 to-transparent pb-2.5 pr-2.5">
            <span className="flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1.5 text-xs font-medium text-muted-foreground shadow-sm transition-colors group-hover:border-primary/50 group-hover:text-foreground">
              <ArrowsOut size={12} aria-hidden />
              Open editor
            </span>
          </div>
        </button>
      </SettingsSection>

      <Dialog
        open={editorOpen}
        onOpenChange={setEditorOpen}
        className="flex h-[90vh] max-h-[90vh] w-[90vw] max-w-4xl flex-col overflow-hidden"
      >
        <DialogContent className="flex min-h-0 flex-1 flex-col p-0">
          <DialogHeader className="shrink-0 border-b px-6 py-4">
            <DialogTitle>Workflow definition (JSON)</DialogTitle>
            <DialogDescription>
              Edit the state machine directly. Leave as{' '}
              <code className="rounded bg-muted px-1 font-mono text-xs">{'{}'}</code> to use the
              built-in default. Changes take effect after saving.
            </DialogDescription>
          </DialogHeader>
          <div className="flex-1 p-6">
            <Suspense fallback={<Skeleton className="h-full w-full" />}>
              {editorOpen && (
                <JsonEditor
                  value={workflowJson}
                  onChange={setWorkflowJson}
                  height="calc(90vh - 210px)"
                />
              )}
            </Suspense>
          </div>
          <DialogFooter className="shrink-0 border-t px-6 py-4">
            <Button variant="outline" onClick={() => setEditorOpen(false)}>
              Cancel
            </Button>
            <Button disabled={isSaving} onClick={onSave}>
              {isSaving ? 'Saving…' : 'Save workflow'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
