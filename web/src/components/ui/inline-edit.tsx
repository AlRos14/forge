import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/cn'

type Props = {
  value: string
  onCommit: (value: string) => void
  className?: string
  inputClassName?: string
  placeholder?: string
}

export function InlineEdit({ value, onCommit, className, inputClassName, placeholder }: Props) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (!editing) setDraft(value)
  }, [value, editing])

  useEffect(() => {
    if (editing) {
      inputRef.current?.select()
    }
  }, [editing])

  function commit() {
    setEditing(false)
    const next = draft.trim()
    if (!next || next === value) {
      setDraft(value)
      return
    }
    onCommit(next)
  }

  function cancel() {
    setEditing(false)
    setDraft(value)
  }

  if (editing) {
    return (
      <input
        ref={inputRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            commit()
          }
          if (e.key === 'Escape') {
            e.preventDefault()
            cancel()
          }
        }}
        placeholder={placeholder}
        className={cn(
          'min-w-0 flex-1 bg-transparent text-sm font-medium outline-none ring-0',
          'rounded border border-ring/50 px-1.5 py-0.5 focus:border-ring',
          inputClassName,
        )}
      />
    )
  }

  return (
    <button
      type="button"
      onClick={() => setEditing(true)}
      title="Click to rename"
      className={cn(
        'min-w-0 flex-1 truncate rounded px-1.5 py-0.5 text-left text-sm font-medium',
        'hover:bg-accent/60 cursor-text transition-colors',
        className,
      )}
    >
      {value || placeholder}
    </button>
  )
}
