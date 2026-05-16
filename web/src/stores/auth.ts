import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import type { AuthResponse, UserResponse } from '@/types/generated'

type AuthState = {
  accessToken: string | null
  refreshToken: string | null
  user: UserResponse | null
  setAuth: (auth: AuthResponse, user: UserResponse) => void
  updateTokens: (auth: AuthResponse) => void
  updateUser: (user: UserResponse) => void
  clearAuth: () => void
}

export const useAuthStore = create<AuthState>()(
  persist(
    (set) => ({
      accessToken: null,
      refreshToken: null,
      user: null,
      setAuth: (auth, user) =>
        set({
          accessToken: auth.access_token,
          refreshToken: auth.refresh_token,
          user,
        }),
      updateTokens: (auth) =>
        set({
          accessToken: auth.access_token,
          refreshToken: auth.refresh_token,
        }),
      updateUser: (user) => set({ user }),
      clearAuth: () =>
        set({
          accessToken: null,
          refreshToken: null,
          user: null,
        }),
    }),
    {
      name: 'forge-auth',
      storage: createJSONStorage(() => window.localStorage),
      partialize: (state) => ({
        accessToken: state.accessToken,
        refreshToken: state.refreshToken,
        user: state.user,
      }),
    },
  ),
)

// Module-level singleton ensures concurrent 401s share one refresh request
let pendingRefresh: Promise<string> | null = null

export async function refreshAccess(): Promise<string> {
  if (pendingRefresh) return pendingRefresh

  const { refreshToken, updateTokens, clearAuth } = useAuthStore.getState()
  if (!refreshToken) {
    clearAuth()
    throw new Error('No refresh token')
  }

  pendingRefresh = (async (): Promise<string> => {
    const response = await fetch('/api/v1/auth/refresh', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ refresh_token: refreshToken }),
    })
    if (!response.ok) {
      clearAuth()
      throw new Error('Token refresh failed')
    }
    const auth = (await response.json()) as AuthResponse
    updateTokens(auth)
    return auth.access_token
  })().finally(() => {
    pendingRefresh = null
  })

  return pendingRefresh
}
