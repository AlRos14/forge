import { expect, request as playwrightRequest, test } from '@playwright/test'
import { authStorageValue, backendAvailable, ensureDefaultAuth } from './auth-utils'

test('a real REST-created Task is loaded and rendered by the web board', async ({ page }) => {
  const backendBaseURL = process.env.FORGE_E2E_BACKEND_BASE_URL ?? 'http://localhost:8080'
  const backend = await playwrightRequest.newContext({ baseURL: backendBaseURL })
  let projectId: string | undefined
  let accessToken: string | undefined

  try {
    expect(
      await backendAvailable(backend),
      `PR15 backend acceptance requires a live Forge backend at ${backendBaseURL}`,
    ).toBe(true)

    const auth = await ensureDefaultAuth(backend)
    expect(
      auth,
      'PR15 backend acceptance must authenticate against Forge; fallback auth is forbidden',
    ).not.toBeNull()
    if (!auth) throw new Error(`Could not authenticate to the Forge backend at ${backendBaseURL}`)
    accessToken = auth.apiToken

    const projectResponse = await backend.post('/api/v1/projects', {
      headers: { authorization: `Bearer ${auth.apiToken}` },
      data: { name: `PR15 backend smoke ${Date.now()}` },
      failOnStatusCode: false,
    })
    expect(
      projectResponse.ok(),
      `REST Project creation failed with ${projectResponse.status()}: ${await projectResponse.text()}`,
    ).toBe(true)
    const project = (await projectResponse.json()) as { id: string }
    projectId = project.id

    const taskTitle = `PR15 REST projected Task ${Date.now()}`
    const taskResponse = await backend.post(`/api/v1/projects/${project.id}/tasks`, {
      headers: { authorization: `Bearer ${auth.apiToken}` },
      data: { title: taskTitle, description: 'Created through the real backend smoke.' },
      failOnStatusCode: false,
    })
    expect(
      taskResponse.ok(),
      `REST Task creation failed with ${taskResponse.status()}: ${await taskResponse.text()}`,
    ).toBe(true)
    const task = (await taskResponse.json()) as { id: string; title: string }
    expect(task.title).toBe(taskTitle)

    await page.addInitScript((value) => {
      window.localStorage.setItem('forge-auth', value)
    }, authStorageValue(auth))

    const boardTasksResponse = page.waitForResponse((response) => {
      const url = new URL(response.url())
      return (
        url.pathname === `/api/v1/projects/${project.id}/tasks` &&
        response.request().method() === 'GET'
      )
    })
    await page.goto(`/projects/${project.id}/board`)
    await expect(page.getByRole('heading', { name: 'Task board' })).toBeVisible()

    const projectedResponse = await boardTasksResponse
    expect(
      projectedResponse.ok(),
      `Web board REST read failed with ${projectedResponse.status()}`,
    ).toBe(true)
    const projectedTasks = (await projectedResponse.json()) as {
      items: Array<{ id: string; title: string }>
    }
    expect(
      projectedTasks.items.some((item) => item.id === task.id && item.title === taskTitle),
    ).toBe(true)
    await expect(page.getByText(taskTitle, { exact: true })).toBeVisible()
  } finally {
    if (projectId) {
      if (accessToken) {
        const cleanup = await backend.delete(`/api/v1/projects/${projectId}`, {
          headers: { authorization: `Bearer ${accessToken}` },
          failOnStatusCode: false,
        })
        expect([204, 404].includes(cleanup.status())).toBe(true)
      }
    }
    await backend.dispose()
  }
})
