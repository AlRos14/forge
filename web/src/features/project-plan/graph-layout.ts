import type { ProjectPlanEdge } from '@/types/generated'

export const NODE_WIDTH = 276
export const NODE_HEIGHT = 222
const COLUMN_GAP = 82
const ROW_GAP = 32
const PADDING = 24

type LayoutNode = { key: string; parallel_level: number }
export type PositionedNode = { key: string; x: number; y: number }

/** Layout only. Availability, graph validation and scheduling belong to the server. */
export function layoutProjectGraph(nodes: LayoutNode[]) {
  const columns = new Map<number, LayoutNode[]>()
  for (const node of nodes) {
    const level = Math.max(0, Math.floor(node.parallel_level))
    columns.set(level, [...(columns.get(level) ?? []), node])
  }
  const levels = [...columns.keys()].sort((a, b) => a - b)
  const positions = new Map<string, PositionedNode>()
  levels.forEach((level, column) => {
    columns.get(level)!.forEach((node, row) => {
      positions.set(node.key, {
        key: node.key,
        x: PADDING + column * (NODE_WIDTH + COLUMN_GAP),
        y: PADDING + row * (NODE_HEIGHT + ROW_GAP),
      })
    })
  })
  const rows = Math.max(1, ...[...columns.values()].map((column) => column.length))
  return {
    positions,
    levels,
    width: PADDING * 2 + Math.max(1, levels.length) * (NODE_WIDTH + COLUMN_GAP) - COLUMN_GAP,
    height: PADDING * 2 + rows * (NODE_HEIGHT + ROW_GAP) - ROW_GAP,
  }
}

export function edgePath(edge: ProjectPlanEdge, positions: Map<string, PositionedNode>) {
  const from = positions.get(edge.predecessor_key)
  const to = positions.get(edge.successor_key)
  if (!from || !to) return null
  const x1 = from.x + NODE_WIDTH
  const y1 = from.y + NODE_HEIGHT / 2
  const x2 = to.x
  const y2 = to.y + NODE_HEIGHT / 2
  const midpoint = (x1 + x2) / 2
  return `M ${x1} ${y1} C ${midpoint} ${y1}, ${midpoint} ${y2}, ${x2} ${y2}`
}

export function formatMinuteRange(min: number | null, max: number | null): string | null {
  if (min === null || max === null) return null
  return min === max ? `${min} min` : `${min}–${max} min`
}
