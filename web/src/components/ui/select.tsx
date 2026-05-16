import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { CaretDown, Check } from '@phosphor-icons/react'
import { cn } from '@/lib/cn'

export type SelectOption = {
  value: string
  label: string
  disabled?: boolean
}

type SelectProps = {
  id?: string
  value: string
  options: SelectOption[]
  onChange: (value: string) => void
  placeholder?: string
  disabled?: boolean
  className?: string
  'aria-label'?: string
  title?: string
}

export function Select({
  id,
  value,
  options,
  onChange,
  placeholder = 'Select...',
  disabled,
  className,
  'aria-label': ariaLabel,
  title,
}: SelectProps) {
  const [open, setOpen] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const dropdownRef = useRef<HTMLDivElement>(null)
  const [style, setStyle] = useState<React.CSSProperties>({})

  const selectedOption = options.find((o) => o.value === value)
  const displayLabel = selectedOption?.label ?? (value || placeholder)
  const showPlaceholder = !selectedOption && !value

  const updatePosition = useCallback(() => {
    if (!triggerRef.current) return
    const rect = triggerRef.current.getBoundingClientRect()
    setStyle({
      position: 'fixed',
      zIndex: 9999,
      top: rect.bottom + 4,
      left: rect.left,
      minWidth: rect.width,
    })
  }, [])

  useEffect(() => {
    if (!open) return
    updatePosition()
    const onMouseDown = (e: globalThis.MouseEvent) => {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(e.target as Node) &&
        triggerRef.current &&
        !triggerRef.current.contains(e.target as Node)
      ) {
        setOpen(false)
      }
    }
    const onScroll = () => updatePosition()
    document.addEventListener('mousedown', onMouseDown)
    window.addEventListener('scroll', onScroll, true)
    return () => {
      document.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('scroll', onScroll, true)
    }
  }, [open, updatePosition])

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (disabled) return
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      setOpen((v) => !v)
    }
    if (e.key === 'Escape') setOpen(false)
    if (e.key === 'ArrowDown' && !open) {
      e.preventDefault()
      setOpen(true)
    }
  }

  return (
    <>
      <button
        ref={triggerRef}
        id={id}
        type="button"
        disabled={disabled}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={title}
        className={cn(
          'flex h-9 w-full cursor-pointer items-center justify-between gap-2 rounded-md border border-input bg-background px-3 py-2 text-ui ring-offset-background transition-colors',
          'hover:bg-accent/40 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2',
          'disabled:cursor-not-allowed disabled:opacity-50',
          className,
        )}
        onKeyDown={handleKeyDown}
        onClick={() => { if (!disabled) { if (!open) updatePosition(); setOpen((v) => !v) } }}
      >
        <span className={cn('truncate text-left', showPlaceholder && 'text-muted-foreground')}>
          {displayLabel}
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
            className="max-h-64 overflow-y-auto rounded-lg border border-border-subtle bg-popover p-1 text-popover-foreground shadow-float animate-slide-in"
          >
            {options.map((option) => (
              <button
                key={option.value}
                role="option"
                type="button"
                disabled={option.disabled}
                aria-selected={option.value === value}
                className={cn(
                  'relative flex w-full cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-ui outline-none transition-colors',
                  'hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground',
                  'disabled:pointer-events-none disabled:opacity-40',
                  option.value === value && 'bg-accent/50',
                )}
                onClick={() => {
                  onChange(option.value)
                  setOpen(false)
                }}
              >
                <Check
                  size={12}
                  className={cn('shrink-0', option.value === value ? 'opacity-100' : 'opacity-0')}
                />
                <span className="truncate">{option.label}</span>
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  )
}
