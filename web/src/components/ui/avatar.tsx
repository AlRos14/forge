import { cn } from '@/lib/cn'

/**
 * Deterministic avatar — generates a unique gradient from the seed string.
 * Same seed always produces the same color pair.
 */

const palette = [
  ['#6366f1', '#818cf8'], // indigo
  ['#8b5cf6', '#a78bfa'], // violet
  ['#ec4899', '#f472b6'], // pink
  ['#f43f5e', '#fb7185'], // rose
  ['#ef4444', '#f87171'], // red
  ['#f97316', '#fb923c'], // orange
  ['#eab308', '#facc15'], // yellow
  ['#22c55e', '#4ade80'], // green
  ['#14b8a6', '#2dd4bf'], // teal
  ['#06b6d4', '#22d3ee'], // cyan
  ['#3b82f6', '#60a5fa'], // blue
  ['#a855f7', '#c084fc'], // purple
]

function hashCode(str: string): number {
  let hash = 0
  for (let i = 0; i < str.length; i++) {
    hash = ((hash << 5) - hash + str.charCodeAt(i)) | 0
  }
  return Math.abs(hash)
}

function getColors(seed: string): [string, string] {
  const index = hashCode(seed) % palette.length
  return palette[index] as [string, string]
}

function getInitial(name: string): string {
  return (name[0] ?? '?').toUpperCase()
}

interface AvatarProps {
  name: string
  seed?: string
  size?: 'xs' | 'sm' | 'md' | 'lg'
  className?: string
}

const sizeClasses = {
  xs: 'h-4 w-4 text-[8px] rounded',
  sm: 'h-6 w-6 text-[10px] rounded-md',
  md: 'h-7 w-7 text-[11px] rounded-md',
  lg: 'h-10 w-10 text-sm rounded-lg',
}

export function Avatar({ name, seed, size = 'md', className }: AvatarProps) {
  const [from, to] = getColors(seed ?? name)
  return (
    <div
      className={cn(
        'flex shrink-0 items-center justify-center font-bold text-white select-none',
        sizeClasses[size],
        className,
      )}
      style={{ background: `linear-gradient(135deg, ${from}, ${to})` }}
      title={name}
    >
      {getInitial(name)}
    </div>
  )
}
