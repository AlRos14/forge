import { cn } from '@/lib/cn'
import type { HTMLAttributes } from 'react'

const variantStyles = {
  default: 'rounded-full border-transparent bg-primary text-primary-foreground',
  secondary: 'rounded-sm border-transparent bg-secondary text-secondary-foreground',
  destructive: 'rounded-full border-transparent bg-destructive text-destructive-foreground',
  outline: 'rounded-sm text-foreground',
}

interface BadgeProps extends HTMLAttributes<HTMLDivElement> {
  variant?: keyof typeof variantStyles
}

export function Badge({ className, variant = 'default', ...props }: BadgeProps) {
  return (
    <div
      className={cn(
        'inline-flex items-center border px-2 py-0.5 text-[11px] font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2',
        variantStyles[variant],
        className,
      )}
      {...props}
    />
  )
}
