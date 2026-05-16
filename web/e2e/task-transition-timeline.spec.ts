import { expect, test } from './fixtures'

test('transition timeline renders on task detail', async ({ page, request }) => {
  const projectsResp = await request.get('/api/v1/projects')
  expect(projectsResp.ok()).toBeTruthy()
  const projects = await projectsResp.json()
  const project = projects.items?.[0]
  test.skip(!project, 'No projects seeded')

  const tasksResp = await request.get(`/api/v1/projects/${project.id}/tasks`)
  expect(tasksResp.ok()).toBeTruthy()
  const tasks = await tasksResp.json()
  const task = tasks.items?.[0]
  test.skip(!task, 'No tasks in demo project')

  await page.goto(`/projects/${project.id}/board?task=${task.id}`)
  await page.waitForLoadState('domcontentloaded')

  const historyTab = page.getByRole('tab', { name: /history|transitions/i })
  if ((await historyTab.count()) > 0) {
    await historyTab.first().click()
  }

  await expect
    .soft(page.getByText(/transition|history|timeline/i).first())
    .toBeVisible({ timeout: 15000 })
})
