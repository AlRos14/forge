import { expect, test, type APIRequestContext } from './fixtures'

type Project = { id: string }

async function firstProject(request: APIRequestContext): Promise<Project> {
  const response = await request.get('/api/v1/projects')
  expect(response.ok()).toBeTruthy()
  const projects = await response.json()
  const project = projects.items?.[0]
  test.skip(!project, 'No projects seeded; run `cargo run -p forge-cli -- --demo`')
  return project
}

test('public navigation and project settings show the target surfaces', async ({
  page,
  request,
}) => {
  const project = await firstProject(request)
  const consoleErrors: string[] = []
  const notFoundResources: string[] = []
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text())
  })
  page.on('pageerror', (error) => consoleErrors.push(`pageerror: ${error.message}`))
  page.on('response', (response) => {
    if (response.status() === 404 && new URL(response.url()).pathname.startsWith('/api/')) {
      notFoundResources.push(response.url())
    }
  })

  await page.goto(`/projects/${project.id}/board`)
  await expect(page.getByRole('heading', { name: 'Task board' })).toBeVisible()
  const primaryNavigation = page.getByRole('navigation', { name: 'Primary navigation' })
  for (const retiredName of [
    'Main Chat',
    'Project Agent Workspace',
    'Agent Workspace',
    'Attention',
    'Mission Control',
  ]) {
    await expect(primaryNavigation.getByRole('link', { name: retiredName, exact: true })).toHaveCount(0)
  }

  const routes: Array<{ path: string; visible: string }> = [
    { path: `/projects/${project.id}/tasks`, visible: 'Tasks' },
    { path: '/agents', visible: 'Agent Settings' },
    { path: '/operations', visible: 'Operations' },
    { path: '/settings', visible: 'System Settings' },
    { path: `/projects/${project.id}/settings`, visible: 'Settings' },
  ]
  for (const route of routes) {
    await page.goto(route.path)
    await expect(page.getByText(route.visible, { exact: true }).first()).toBeVisible({
      timeout: 15_000,
    })
  }

  await page.goto(`/projects/${project.id}/settings`)
  for (const tab of [
    { nav: 'General', heading: 'General' },
    { nav: 'Repos', heading: 'Primary Repository' },
    { nav: 'Members', heading: 'Members' },
    { nav: 'MCP', heading: 'MCP' },
    { nav: 'Hooks', heading: 'Project hooks' },
    { nav: 'Danger zone', heading: 'Danger zone' },
  ]) {
    await page.getByRole('link', { name: tab.nav, exact: true }).click()
    await expect(page.getByRole('heading', { name: tab.heading, exact: true })).toBeVisible({
      timeout: 15_000,
    })
  }

  await page.goto('/settings')
  for (const tab of ['Server', 'Agent', 'Paths']) {
    await page.getByRole('link', { name: tab, exact: true }).click()
    await expect(page.getByRole('heading', { name: tab, exact: true })).toBeVisible({
      timeout: 15_000,
    })
  }

  expect({ consoleErrors, notFoundResources }).toEqual({ consoleErrors: [], notFoundResources: [] })
})
