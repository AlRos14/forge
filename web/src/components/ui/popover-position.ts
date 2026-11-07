/** Keep portalled controls within the viewport, including dialog edges. */
export function popoverPosition(rect: DOMRect, preferredWidth = rect.width): React.CSSProperties {
  const gutter = 8
  const gap = 4
  const width = Math.min(preferredWidth, window.innerWidth - gutter * 2)
  const below = window.innerHeight - rect.bottom - gutter - gap
  const above = rect.top - gutter - gap
  const opensAbove = below < 160 && above > below
  return {
    position: 'fixed',
    zIndex: 9999,
    width,
    left: Math.max(gutter, Math.min(rect.left, window.innerWidth - width - gutter)),
    ...(opensAbove ? { bottom: window.innerHeight - rect.top + gap } : { top: rect.bottom + gap }),
    maxHeight: Math.max(0, Math.min(256, opensAbove ? above : below)),
  }
}
