import { create } from 'zustand'

export type BoardFilters = {
  agentIds: string[]
  priorityMin?: number
  priorityMax?: number
  blockedOnly: boolean
  q: string
  includeCancelled: boolean
  includeArchived: boolean
}

type FilterState = BoardFilters & {
  setFilters: (patch: Partial<BoardFilters>) => void
  resetFilters: () => void
}

const DEFAULT_FILTERS: BoardFilters = {
  agentIds: [],
  blockedOnly: false,
  q: '',
  includeCancelled: false,
  includeArchived: false,
}

export const useFilterStore = create<FilterState>((set) => ({
  ...DEFAULT_FILTERS,
  setFilters: (patch) => set((prev) => ({ ...prev, ...patch })),
  resetFilters: () => set(DEFAULT_FILTERS),
}))

export const filterDefaults = DEFAULT_FILTERS
