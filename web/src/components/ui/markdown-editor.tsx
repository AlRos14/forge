import CodeMirror from '@uiw/react-codemirror'
import { markdown } from '@codemirror/lang-markdown'
import { oneDark } from '@codemirror/theme-one-dark'
import { EditorView } from '@codemirror/view'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import rehypeSanitize from 'rehype-sanitize'
import { useLayoutStore } from '@/stores/layout'
import { cn } from '@/lib/cn'

const baseTheme = EditorView.theme({
  '&': {
    fontSize: '13px',
    fontFamily: 'ui-sans-serif, system-ui, -apple-system, sans-serif',
  },
  '.cm-scroller': { fontFamily: 'inherit', lineHeight: '1.6' },
  '.cm-focused': { outline: 'none' },
  '&.cm-focused': { outline: 'none' },
  '.cm-line': { padding: '0 8px' },
  '.cm-content': { padding: '8px 0' },
})

const lightTheme = EditorView.theme({
  '&': { backgroundColor: 'transparent' },
  '.cm-gutters': { display: 'none' },
})

const darkTheme = EditorView.theme({
  '&': { backgroundColor: 'transparent' },
  '.cm-gutters': { display: 'none' },
})

interface MarkdownEditorProps {
  value: string
  onChange: (value: string) => void
  onKeyDown?: (event: React.KeyboardEvent) => void
  placeholder?: string
  minHeight?: string
  autoFocus?: boolean
  className?: string
}

export function MarkdownEditor({
  value,
  onChange,
  onKeyDown,
  placeholder,
  minHeight = '120px',
  autoFocus = false,
  className,
}: MarkdownEditorProps) {
  const theme = useLayoutStore((s) => s.theme)

  return (
    <div
      className={cn(
        'overflow-hidden rounded-md border border-input bg-background focus-within:ring-2 focus-within:ring-ring focus-within:ring-offset-0',
        className,
      )}
      onKeyDown={onKeyDown}
    >
      <CodeMirror
        value={value}
        placeholder={placeholder}
        autoFocus={autoFocus}
        extensions={[markdown(), baseTheme, theme === 'dark' ? darkTheme : lightTheme]}
        theme={theme === 'dark' ? oneDark : 'light'}
        minHeight={minHeight}
        onChange={onChange}
        aria-label="Markdown editor"
        basicSetup={{
          lineNumbers: false,
          foldGutter: false,
          highlightActiveLine: false,
          highlightActiveLineGutter: false,
        }}
      />
    </div>
  )
}

interface MarkdownViewProps {
  content: string
  className?: string
}

export function MarkdownView({ content, className }: MarkdownViewProps) {
  return (
    <div className={cn('prose prose-sm dark:prose-invert max-w-none', className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]}>
        {content}
      </ReactMarkdown>
    </div>
  )
}
