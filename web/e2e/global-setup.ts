import { request as playwrightRequest } from '@playwright/test'
import { backendAvailable, ensureDefaultAuth } from './auth-utils'

export default async function globalSetup() {
  const baseURL = process.env.FORGE_E2E_BACKEND_BASE_URL ?? 'http://localhost:8080'
  const request = await playwrightRequest.newContext({ baseURL })

  try {
    if (!(await backendAvailable(request))) {
      console.warn(`[e2e auth] Backend unavailable at ${baseURL}; using fixture fallback auth.`)
      return
    }

    await ensureDefaultAuth(request)
  } finally {
    await request.dispose()
  }
}
