import CodeMirror from '@uiw/react-codemirror'
import { json } from '@codemirror/lang-json'
import { StreamLanguage } from '@codemirror/language'
import { shell } from '@codemirror/legacy-modes/mode/shell'
import { EditorState, type Extension } from '@codemirror/state'
import { oneDark } from '@codemirror/theme-one-dark'
import { EditorView } from '@codemirror/view'
import { useLayoutStore } from '@/stores/layout'

// ── Shared themes ──────────────────────────────────────────────────────────

const baseTheme = EditorView.theme({
  '&': {
    fontSize: '12px',
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
  },
  '.cm-scroller': { fontFamily: 'inherit' },
  '.cm-focused': { outline: 'none' },
  '&.cm-focused': { outline: '2px solid hsl(var(--ring))', outlineOffset: '2px' },
})

const lightTheme = EditorView.theme({
  '&': { backgroundColor: 'hsl(var(--background))' },
})

// ── Forge env-var autocomplete ──────────────────────────────────────────────
// Registered via languageData so it merges with the shell language's own
// completions (keywords, snippets) rather than replacing them.

const FORGE_ENV_VARS = [
  { name: 'FORGE_EVENT',         detail: 'Lifecycle event name' },
  { name: 'FORGE_TASK_ID',       detail: 'Task UUID' },
  { name: 'FORGE_TASK_TITLE',    detail: 'Task title' },
  { name: 'FORGE_TASK_STATUS',   detail: 'Current task status' },
  { name: 'FORGE_PROJECT_ID',    detail: 'Project UUID' },
  { name: 'FORGE_REPO_PATH',     detail: 'Path to the repository root' },
  { name: 'FORGE_WORKTREE_PATH', detail: 'Path to the agent worktree' },
]

function forgeEnvCompletion(context: { matchBefore: (re: RegExp) => { from: number; text: string } | null }) {
  const match = context.matchBefore(/\$\w*/)
  if (!match) return null
  const typed = match.text.slice(1)
  return {
    from: match.from + 1,
    options: FORGE_ENV_VARS
      .filter(({ name }) => name.startsWith(typed.toUpperCase()))
      .map(({ name, detail }) => ({ label: name, detail, type: 'variable', boost: 1 })),
  }
}

const forgeEnvCompletionExtension = EditorState.languageData.of(() => [
  { autocomplete: forgeEnvCompletion },
])

// ── Shared editor base ──────────────────────────────────────────────────────

interface EditorProps {
  value: string
  onChange: (value: string) => void
  height?: string
  minHeight?: string
}

function BaseEditor({
  value,
  onChange,
  height,
  minHeight = '300px',
  extensions,
  ariaLabel,
}: EditorProps & { extensions: Extension | Extension[]; ariaLabel: string }) {
  const theme = useLayoutStore((s) => s.theme)

  return (
    <div className="overflow-hidden rounded-md border border-border">
      <CodeMirror
        value={value}
        extensions={[
          ...(Array.isArray(extensions) ? extensions : [extensions]),
          baseTheme,
          theme === 'dark' ? EditorView.theme({}) : lightTheme,
        ]}
        theme={theme === 'dark' ? oneDark : 'light'}
        height={height}
        minHeight={height ? undefined : minHeight}
        onChange={onChange}
        aria-label={ariaLabel}
      />
    </div>
  )
}

// ── Public editors ──────────────────────────────────────────────────────────

export function JsonEditor(props: EditorProps) {
  return <BaseEditor {...props} extensions={[json()]} ariaLabel="JSON editor" />
}

export function ShellEditor(props: EditorProps) {
  return (
    <BaseEditor
      {...props}
      extensions={[StreamLanguage.define(shell), EditorView.lineWrapping, forgeEnvCompletionExtension]}
      ariaLabel="Shell command editor"
    />
  )
}
