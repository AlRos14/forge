export function formatRuntimeSeconds(value?: number | null): string {
  if (value == null || !Number.isFinite(value)) return '-'
  const totalSeconds = Math.max(0, Math.floor(value))
  if (totalSeconds < 60) return `${totalSeconds}s`
  const minutes = Math.floor(totalSeconds / 60)
  if (minutes < 60) {
    const seconds = totalSeconds % 60
    return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`
  }
  const hours = Math.floor(minutes / 60)
  const remainingMinutes = minutes % 60
  if (hours < 24) return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`
  const days = Math.floor(hours / 24)
  const remainingHours = hours % 24
  return remainingHours > 0 ? `${days}d ${remainingHours}h` : `${days}d`
}

export function formatTokenCount(value?: number | null, compact = false): string {
  if (value == null || !Number.isFinite(value)) return '-'
  if (compact) {
    return new Intl.NumberFormat(undefined, {
      notation: 'compact',
      maximumFractionDigits: value >= 10_000 ? 1 : 0,
    }).format(value)
  }
  return Math.max(0, Math.trunc(value)).toLocaleString()
}

export function formatCostUsd(value?: number | null): string {
  if (value == null || !Number.isFinite(value)) return '-'
  if (value === 0) return '$0.00'
  return value < 0.01 ? `$${value.toFixed(4)}` : `$${value.toFixed(2)}`
}
