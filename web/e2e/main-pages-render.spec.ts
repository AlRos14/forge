import { expect, test, type APIRequestContext } from './fixtures'

type Project = {
  id: string
}

async function firstProject(request: APIRequestContext): Promise<Project> {
  const response = await request.get('/api/v1/projects')
  expect(response.ok()).toBeTruthy()
  const projects = await response.json()
  const project = projects.items?.[0]
  test.skip(!project, 'No projects seeded; run `cargo run -p forge-cli -- --demo`')
  return project
}

function isExpectedUnboundMainAgent(url: string): boolean {
  return new URL(url).pathname === '/api/v1/account/main-agent'
}

test('main pages and settings tabs render', async ({ page, request }) => {
  const project = await firstProject(request)
  const consoleErrors: string[] = []
  const notFoundResources: string[] = []
  page.on('console', (message) => {
    if (message.type() === 'error') {
      const location = message.location()
      if (location.url && isExpectedUnboundMainAgent(location.url)) return
      const suffix = location.url ? ` (${location.url}:${location.lineNumber})` : ''
      consoleErrors.push(`${message.text()}${suffix}`)
    }
  })
  page.on('pageerror', (error) => consoleErrors.push(`pageerror: ${error.message}`))
  page.on('response', (response) => {
    if (response.status() === 404 && !isExpectedUnboundMainAgent(response.url())) {
      notFoundResources.push(response.url())
    }
  })

  const routes: Array<{ path: string; visibleText: string | RegExp }> = [
    { path: `/projects/${project.id}/board`, visibleText: /^Todo$/ },
    { path: `/projects/${project.id}/tasks`, visibleText: /^Tasks$/ },
    { path: `/projects/${project.id}/chat`, visibleText: /^.* Agent Workspace$/ },
    { path: '/chat', visibleText: /^Main Chat$/ },
    { path: '/agents', visibleText: /^Agent Settings$/ },
    { path: '/daemons', visibleText: /^Runtimes$/ },
    { path: '/operations', visibleText: /^Operations$/ },
    { path: '/settings', visibleText: /^System Settings$/ },
    { path: `/projects/${project.id}/settings`, visibleText: /^Settings$/ },
  ]

  for (const route of routes) {
    await page.goto(route.path)
    await page.waitForLoadState('domcontentloaded')
    await expect(page.getByText(route.visibleText).first()).toBeVisible({ timeout: 15000 })
  }

  await page.goto(`/projects/${project.id}/settings`)
  for (const tab of [
    { nav: 'General', heading: 'General' },
    { nav: 'Repos', heading: 'Primary Repository' },
    { nav: 'Members', heading: 'Members' },
    { nav: 'MCP', heading: 'MCP' },
    { nav: 'Hooks', heading: 'Lifecycle Hooks' },
    { nav: 'Analytics', heading: 'Analytics' },
    { nav: 'Workflow', heading: 'Workflow definition' },
    { nav: 'Danger zone', heading: 'Danger zone' },
  ]) {
    await page.getByRole('link', { name: tab.nav, exact: true }).click()
    await expect(page.getByRole('heading', { name: tab.heading, exact: true })).toBeVisible({
      timeout: 15000,
    })
  }

  await page.getByRole('link', { name: 'MCP', exact: true }).click()
  const projectMcp = page.getByText('Project MCP', { exact: true })
  const noLocalRepo = page.getByText('No local repository configured', { exact: true })
  await expect(projectMcp.or(noLocalRepo)).toBeVisible()
  if (await projectMcp.isVisible()) {
    await expect(page.getByLabel('MCP client')).toBeVisible()
    await expect(page.getByText(/Not installed|Installed/).first()).toBeVisible({ timeout: 15000 })
    await expect(
      page
        .getByText(/\.claude\/settings\.json|\.codex\/config\.toml|\.cursor\/mcp\.json|\/mcp\?project_id=/)
        .first(),
    ).toBeVisible()
  } else {
    await expect(page.getByRole('link', { name: 'Go to Repos', exact: true })).toBeVisible()
  }

  await page.goto('/settings')
  for (const tab of ['Server', 'Agent', 'Paths']) {
    await page.getByRole('link', { name: tab, exact: true }).click()
    await expect(page.getByRole('heading', { name: tab, exact: true })).toBeVisible({
      timeout: 15000,
    })
  }

  expect({ consoleErrors, notFoundResources }).toEqual({ consoleErrors: [], notFoundResources: [] })
})
