import { forwardRef, type InputHTMLAttributes } from 'react'
import { cn } from '@/lib/cn'

type SwitchProps = Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>

export const Switch = forwardRef<HTMLInputElement, SwitchProps>(
  ({ className, ...props }, ref) => {
    return (
      <label className={cn('relative inline-flex h-6 w-11 cursor-pointer items-center', className)}>
        <input ref={ref} type="checkbox" className="peer sr-only" {...props} />
        <div className="h-6 w-11 rounded-full bg-input transition-colors peer-checked:bg-primary peer-focus-visible:ring-2 peer-focus-visible:ring-ring peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-background peer-disabled:cursor-not-allowed peer-disabled:opacity-50" />
        <div className="absolute left-1 top-1 h-4 w-4 rounded-full bg-background transition-transform peer-checked:translate-x-5" />
      </label>
    )
  },
)
Switch.displayName = 'Switch'
