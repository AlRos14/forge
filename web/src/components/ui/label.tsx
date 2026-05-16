import { forwardRef, type LabelHTMLAttributes } from 'react'
import { cn } from '@/lib/cn'

export const Label = forwardRef<HTMLLabelElement, LabelHTMLAttributes<HTMLLabelElement>>(
  ({ className, ...props }, ref) => {
    return (
      <label
        ref={ref}
        className={cn(
          'font-mono text-[10px] leading-tight font-semibold uppercase tracking-[0.8px] text-muted-foreground peer-disabled:cursor-not-allowed peer-disabled:opacity-70',
          className,
        )}
        {...props}
      />
    )
  },
)
Label.displayName = 'Label'
