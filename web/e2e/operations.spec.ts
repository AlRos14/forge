import { expect, test } from './fixtures'

test('loads operations without console errors', async ({ page }) => {
  const consoleErrors: string[] = []
  page.on('console', (message) => {
    if (message.type() === 'error') {
      consoleErrors.push(message.text())
    }
  })

  await page.route('**/api/v1/events*', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/event-stream',
      body: '',
    }),
  )
  await page.route('**/api/v1/operations/status', (route) =>
    route.fulfill({
      json: {
        overall_severity: 'healthy',
        active_executions: [],
        blocked_tasks: [],
        daemon_issues: [],
        daemon_pressure: [],
        agent_pressure: [],
        workspace_cleanup: [],
        retry_pressure: [],
        usage_summary: null,
        recent_errors: [],
        computed_at: '2026-04-29T12:00:00Z',
      },
    }),
  )
  await page.route('**/api/v1/projects', (route) =>
    route.fulfill({
      json: {
        items: [
          {
            id: 'project-operations-smoke',
            name: 'Operations Smoke',
            settings: {},
            workflow_template_name: null,
            default_review_config: { ci_steps: [], review_prompt: null },
            paused: false,
            created_at: '2026-04-29T12:00:00Z',
            updated_at: '2026-04-29T12:00:00Z',
          },
        ],
        has_more: false,
      },
    }),
  )
  await page.route('**/api/v1/projects/project-operations-smoke/tasks*', (route) =>
    route.fulfill({
      json: {
        items: [],
        has_more: false,
      },
    }),
  )
  await page.route('**/api/v1/notifications/unread-count*', (route) =>
    route.fulfill({ json: { count: 0 } }),
  )
  await page.route('**/api/v1/notifications?*', (route) =>
    route.fulfill({ json: { items: [], has_more: false } }),
  )

  await page.goto('/operations')

  await expect(page.getByRole('heading', { name: 'Operations' })).toBeVisible()
  expect(consoleErrors).toEqual([])
})
