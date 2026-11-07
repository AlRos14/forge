import { test, expect } from './fixtures'
for (const width of [320, 390, 768, 1280, 1440]) {
  test(`route states ${width}`, async ({ page, request }, info) => {
    test.setTimeout(120000)
    await page.setViewportSize({ width, height: 844 })
    const projects = await request.get('/api/v1/projects')
    expect(projects.ok()).toBe(true)
    const project = (await projects.json()).items[0]
    const tasks = await request.get(`/api/v1/projects/${project.id}/tasks`)
    expect(tasks.ok()).toBe(true)
    const task = (await tasks.json()).items[0]
    for (const tab of ['overview', 'work', 'executions', 'review', 'terminal', 'history']) {
      await page.goto(`/tasks/${task.id}/${tab}`)
      await expect(page.getByRole('heading').first()).toBeVisible()
      await page.screenshot({ path: info.outputPath(`task-${tab}.png`), fullPage: true })
      // Domain panels belong to A: record rather than hide their overflow.
      await info.attach(`task-${tab}-overflow`, {
        body: JSON.stringify(
          await page.evaluate(() => ({
            page: document.documentElement.scrollWidth - innerWidth,
            main:
              document.querySelector('#main-content')!.scrollWidth -
              document.querySelector('#main-content')!.clientWidth,
          })),
        ),
        contentType: 'application/json',
      })
    }
  })
}
