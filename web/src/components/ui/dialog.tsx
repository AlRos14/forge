import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/cn'

type DialogA11yContextValue = {
  titleId: string
  descriptionId: string
  registerDescription: (present: boolean) => void
}

const DialogA11yContext = createContext<DialogA11yContextValue | null>(null)

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'area[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  'iframe',
  '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

interface DialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  children: ReactNode
  className?: string
  ariaLabel?: string
}

export function Dialog({ open, onOpenChange, children, className, ariaLabel }: DialogProps) {
  const dialogId = useId().replaceAll(':', '')
  const titleId = `${dialogId}-title`
  const descriptionId = `${dialogId}-description`
  const [hasDescription, setHasDescription] = useState(false)
  const contentRef = useRef<HTMLDivElement>(null)
  const restoreFocusRef = useRef<HTMLElement | null>(null)
  const onOpenChangeRef = useRef(onOpenChange)

  useEffect(() => {
    onOpenChangeRef.current = onOpenChange
  }, [onOpenChange])

  const registerDescription = useCallback((present: boolean) => {
    setHasDescription(present)
  }, [])
  const a11yContext = useMemo(
    () => ({ titleId, descriptionId, registerDescription }),
    [descriptionId, registerDescription, titleId],
  )

  useEffect(() => {
    if (!open) return
    restoreFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null

    const focusInitialElement = () => {
      const content = contentRef.current
      if (!content) return
      const firstFocusable = content.querySelector<HTMLElement>(FOCUSABLE_SELECTOR)
      ;(firstFocusable ?? content).focus()
    }
    const focusTimer = window.setTimeout(focusInitialElement, 0)

    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        onOpenChangeRef.current(false)
        return
      }
      if (event.key !== 'Tab') return

      const content = contentRef.current
      if (!content) return
      const focusable = Array.from(
        content.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
      ).filter((element) => element.getClientRects().length > 0)
      if (focusable.length === 0) {
        event.preventDefault()
        content.focus()
        return
      }

      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }

    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    document.addEventListener('keydown', onKey)
    return () => {
      window.clearTimeout(focusTimer)
      document.body.style.overflow = prev
      document.removeEventListener('keydown', onKey)
      const restoreTarget = restoreFocusRef.current
      restoreFocusRef.current = null
      if (restoreTarget?.isConnected) {
        window.setTimeout(() => restoreTarget.focus(), 0)
      }
    }
  }, [open])

  if (!open) return null

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div
        className="absolute inset-0 bg-black/50"
        aria-hidden="true"
        onClick={() => onOpenChangeRef.current(false)}
      />
      <DialogA11yContext.Provider value={a11yContext}>
        <div
          ref={contentRef}
          tabIndex={-1}
          className={cn(
            'relative z-10 max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-lg border bg-background p-0 text-foreground shadow-lg',
            'animate-in fade-in-0 zoom-in-95',
            className,
          )}
          role="dialog"
          aria-modal="true"
          aria-label={ariaLabel}
          aria-labelledby={ariaLabel ? undefined : titleId}
          aria-describedby={hasDescription ? descriptionId : undefined}
          onClick={(e) => e.stopPropagation()}
        >
          {children}
        </div>
      </DialogA11yContext.Provider>
    </div>,
    document.body,
  )
}

export function DialogContent({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  return <div className={cn('p-6', className)}>{children}</div>
}

export function DialogHeader({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={cn('flex flex-col space-y-1.5 text-center sm:text-left', className)}>
      {children}
    </div>
  )
}

export function DialogTitle({ className, children }: { className?: string; children: ReactNode }) {
  const context = useContext(DialogA11yContext)
  return (
    <h2
      id={context?.titleId}
      className={cn('text-lg font-semibold leading-none tracking-tight', className)}
    >
      {children}
    </h2>
  )
}

export function DialogDescription({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  const context = useContext(DialogA11yContext)
  useEffect(() => {
    if (!context) return
    context.registerDescription(true)
    return () => context.registerDescription(false)
  }, [context])
  return (
    <p id={context?.descriptionId} className={cn('text-sm text-muted-foreground', className)}>
      {children}
    </p>
  )
}

export function DialogFooter({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={cn('flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2', className)}>
      {children}
    </div>
  )
}
