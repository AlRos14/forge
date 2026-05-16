import { expect, test } from './fixtures'

test('board page renders workflow columns', async ({ page, request }) => {
  const projectsResp = await request.get('/api/v1/projects')
  expect(projectsResp.ok()).toBeTruthy()
  const projects = await projectsResp.json()
  const project = projects.items?.[0]
  test.skip(!project, 'No projects seeded; run `cargo run -p forge-cli -- --demo`')

  await page.goto(`/projects/${project.id}/board`)
  await page.waitForLoadState('domcontentloaded')

  for (const label of ['Todo', 'In Progress', 'Review', 'Done']) {
    await expect
      .soft(page.getByText(label, { exact: true }).first())
      .toBeVisible({ timeout: 15000 })
  }
})
