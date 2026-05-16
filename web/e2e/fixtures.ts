import { expect, request as playwrightRequest, test as base } from '@playwright/test'
import type { APIRequestContext, APIResponse, Page } from '@playwright/test'
import { authStorageValue, ensureDefaultAuth, fallbackAuth, type E2EAuth } from './auth-utils'

let defaultAuthPromise: Promise<E2EAuth> | null = null

async function defaultAuth(): Promise<E2EAuth> {
  defaultAuthPromise ??= (async () => {
    const baseURL = process.env.FORGE_E2E_BACKEND_BASE_URL ?? 'http://localhost:8080'
    const request = await playwrightRequest.newContext({ baseURL })
    try {
      return (await ensureDefaultAuth(request)) ?? fallbackAuth()
    } finally {
      await request.dispose()
    }
  })()
  return defaultAuthPromise
}

export const test = base.extend<{ e2eAuth: E2EAuth }>({
  e2eAuth: async ({ baseURL }, run) => {
    void baseURL
    await run(await defaultAuth())
  },

  request: async ({ playwright, baseURL, e2eAuth }, run) => {
    const request = await playwright.request.newContext({
      baseURL,
      extraHTTPHeaders: {
        authorization: `Bearer ${e2eAuth.apiToken}`,
      },
    })
    await run(request)
    await request.dispose()
  },

  page: async ({ page, e2eAuth }, run) => {
    const storageValue = authStorageValue(e2eAuth)
    await page.addInitScript((value) => {
      window.localStorage.setItem('forge-auth', value)
    }, storageValue)
    await run(page)
  },
})

export { expect }
export type { APIRequestContext, APIResponse, Page }
