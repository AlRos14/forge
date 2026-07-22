import { useCallback, useMemo } from 'react'
import { assembleBoardSnapshot } from './board-ordering'
import { useBoardTasks, type BoardTaskSearch } from './board-api'

export function useBoardData(projectId: string, search: BoardTaskSearch) {
  const query = useBoardTasks(projectId, search)
  const snapshot = useMemo(
    () => assembleBoardSnapshot(query.data?.pages),
    [query.data?.pages],
  )
  const refresh = useCallback(async () => {
    const result = await query.refetch()
    return assembleBoardSnapshot(result.data?.pages)
  }, [query])

  return { query, snapshot, refresh }
}
