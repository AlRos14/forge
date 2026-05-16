import { createContext, useContext, useState, type ReactNode } from 'react'
import { cn } from '@/lib/cn'

interface TabsCtx {
  value: string
  onValueChange: (v: string) => void
}

const Ctx = createContext<TabsCtx>({ value: '', onValueChange: () => {} })

export function Tabs({ defaultValue, value: controlledValue, onValueChange, children, className }: {
  defaultValue?: string
  value?: string
  onValueChange?: (v: string) => void
  children: ReactNode
  className?: string
}) {
  const [internal, setInternal] = useState(defaultValue ?? '')
  const value = controlledValue ?? internal
  const onChange = onValueChange ?? setInternal

  return (
    <Ctx.Provider value={{ value, onValueChange: onChange }}>
      <div className={className}>{children}</div>
    </Ctx.Provider>
  )
}

export function TabsList({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn('inline-flex h-10 items-center justify-center rounded-md bg-muted p-1 text-muted-foreground', className)}>
      {children}
    </div>
  )
}

export function TabsTrigger({ value, children, className }: { value: string; children: ReactNode; className?: string }) {
  const ctx = useContext(Ctx)
  return (
    <button
      type="button"
      className={cn(
        'inline-flex items-center justify-center whitespace-nowrap rounded-sm px-3 py-1.5 text-sm font-medium ring-offset-background transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50',
        ctx.value === value && 'bg-background text-foreground shadow-sm',
        className,
      )}
      onClick={() => ctx.onValueChange(value)}
    >
      {children}
    </button>
  )
}

export function TabsContent({ value, children, className }: { value: string; children: ReactNode; className?: string }) {
  const ctx = useContext(Ctx)
  if (ctx.value !== value) return null
  return (
    <div className={cn('mt-2 ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2', className)}>
      {children}
    </div>
  )
}
