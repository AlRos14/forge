import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { CaretDown, Check, MagnifyingGlass } from '@phosphor-icons/react'
import { cn } from '@/lib/cn'

export type ComboSelectOption = {
  value: string
  label: string
  description?: string
  group?: string
}

type ComboSelectProps = {
  id?: string
  value: string | null
  options: ComboSelectOption[]
  onChange: (value: string | null) => void
  placeholder?: string
  allowCustom?: boolean
  disabled?: boolean
  isLoading?: boolean
  className?: string
  'aria-label'?: string
}

export function ComboSelect({
  id,
  value,
  options,
  onChange,
  placeholder = 'Default',
  allowCustom = false,
  disabled,
  isLoading,
  className,
  'aria-label': ariaLabel,
}: ComboSelectProps) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const triggerRef = useRef<HTMLButtonElement>(null)
  const dropdownRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const [style, setStyle] = useState<React.CSSProperties>({})

  const selectedOption = value ? options.find((o) => o.value === value) : null
  const displayLabel = selectedOption?.label ?? value ?? placeholder
  const showPlaceholder = !value

  const updatePosition = useCallback(() => {
    if (!triggerRef.current) return
    const rect = triggerRef.current.getBoundingClientRect()
    setStyle({
      position: 'fixed',
      zIndex: 9999,
      top: rect.bottom + 4,
      left: rect.left,
      minWidth: Math.max(rect.width, 200),
    })
  }, [])

  useEffect(() => {
    if (!open) { setQuery(''); return }
    updatePosition()
    requestAnimationFrame(() => searchRef.current?.focus())

    const onMouseDown = (e: globalThis.MouseEvent) => {
      if (
        dropdownRef.current && !dropdownRef.current.contains(e.target as Node) &&
        triggerRef.current && !triggerRef.current.contains(e.target as Node)
      ) setOpen(false)
    }
    const onScroll = () => updatePosition()
    document.addEventListener('mousedown', onMouseDown)
    window.addEventListener('scroll', onScroll, true)
    return () => {
      document.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('scroll', onScroll, true)
    }
  }, [open, updatePosition])

  const filtered = query
    ? options.filter(
        (o) =>
          o.label.toLowerCase().includes(query.toLowerCase()) ||
          o.value.toLowerCase().includes(query.toLowerCase()),
      )
    : options

  const showCustomOption =
    allowCustom &&
    query.trim() &&
    !options.some((o) => o.value === query.trim() || o.label.toLowerCase() === query.trim().toLowerCase())

  const selectValue = (v: string | null) => {
    onChange(v)
    setOpen(false)
  }

  const handleSearchKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') { setOpen(false); return }
    if (e.key === 'Enter') {
      if (e.nativeEvent.isComposing) return
      if (filtered.length === 1) { selectValue(filtered[0].value); return }
      if (showCustomOption) { selectValue(query.trim()); return }
      if (filtered.length === 0 && !allowCustom) return
    }
  }

  // Group options by group key
  const groups = filtered.reduce<Record<string, ComboSelectOption[]>>((acc, opt) => {
    const g = opt.group ?? ''
    if (!acc[g]) acc[g] = []
    acc[g].push(opt)
    return acc
  }, {})
  const groupKeys = Object.keys(groups)

  return (
    <>
      <button
        ref={triggerRef}
        id={id}
        type="button"
        disabled={disabled || isLoading}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        className={cn(
          'flex h-9 w-full cursor-pointer items-center justify-between gap-2 rounded-md border border-input bg-background px-3 py-2 text-ui ring-offset-background transition-colors',
          'hover:bg-accent/40 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2',
          'disabled:cursor-not-allowed disabled:opacity-50',
          className,
        )}
        onClick={() => { if (!disabled && !isLoading) { if (!open) updatePosition(); setOpen((v) => !v) } }}
      >
        <span className={cn('truncate text-left', showPlaceholder && 'text-muted-foreground')}>
          {isLoading ? 'Loading...' : displayLabel}
        </span>
        <CaretDown
          size={12}
          className={cn('shrink-0 text-muted-foreground transition-transform', open && 'rotate-180')}
        />
      </button>

      {open &&
        createPortal(
          <div
            ref={dropdownRef}
            role="listbox"
            style={style}
            className="flex flex-col overflow-hidden rounded-lg border border-border-subtle bg-popover text-popover-foreground shadow-float animate-slide-in"
          >
            {/* Search */}
            <div className="flex items-center gap-2 border-b border-border-subtle px-2 py-1.5">
              <MagnifyingGlass size={12} className="shrink-0 text-muted-foreground" />
              <input
                ref={searchRef}
                type="text"
                value={query}
                placeholder={allowCustom ? 'Search or type a custom value…' : 'Search…'}
                className="min-w-0 flex-1 bg-transparent py-0.5 text-xs outline-none placeholder:text-muted-foreground"
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={handleSearchKey}
              />
            </div>

            {/* Options */}
            <div className="max-h-56 overflow-y-auto p-1">
              {/* Default / clear option */}
              {!query && (
                <button
                  type="button"
                  role="option"
                  aria-selected={!value}
                  className={cn(
                    'relative flex w-full cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-ui outline-none transition-colors',
                    'hover:bg-accent hover:text-accent-foreground',
                    !value && 'bg-accent/50',
                  )}
                  onClick={() => selectValue(null)}
                >
                  <Check size={12} className={cn('shrink-0', !value ? 'opacity-100' : 'opacity-0')} />
                  <span className="text-muted-foreground">{placeholder}</span>
                </button>
              )}

              {groupKeys.map((groupKey) => (
                <div key={groupKey}>
                  {groupKey && (
                    <p className="mt-1 px-2 pb-1 text-micro font-semibold uppercase tracking-wider text-muted-foreground/60">
                      {groupKey}
                    </p>
                  )}
                  {groups[groupKey].map((opt) => (
                    <button
                      key={opt.value}
                      type="button"
                      role="option"
                      aria-selected={opt.value === value}
                      className={cn(
                        'relative flex w-full cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-ui outline-none transition-colors',
                        'hover:bg-accent hover:text-accent-foreground',
                        opt.value === value && 'bg-accent/50',
                      )}
                      onClick={() => selectValue(opt.value)}
                    >
                      <Check size={12} className={cn('shrink-0', opt.value === value ? 'opacity-100' : 'opacity-0')} />
                      <span className="min-w-0">
                        <span className="block truncate">{opt.label}</span>
                        {opt.description && (
                          <span className="block truncate text-[10px] text-muted-foreground">{opt.description}</span>
                        )}
                      </span>
                    </button>
                  ))}
                </div>
              ))}

              {filtered.length === 0 && !showCustomOption && (
                <p className="px-2 py-3 text-center text-xs text-muted-foreground">No matches</p>
              )}

              {showCustomOption && (
                <button
                  type="button"
                  className="relative flex w-full cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-ui outline-none transition-colors hover:bg-accent hover:text-accent-foreground"
                  onClick={() => selectValue(query.trim())}
                >
                  <Check size={12} className="shrink-0 opacity-0" />
                  <span className="truncate text-muted-foreground">
                    Use &ldquo;<span className="font-mono text-foreground">{query.trim()}</span>&rdquo;
                  </span>
                </button>
              )}
            </div>
          </div>,
          document.body,
        )}
    </>
  )
}
