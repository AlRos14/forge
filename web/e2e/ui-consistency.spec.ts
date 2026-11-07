import { test, expect } from './fixtures'

const routes = [
  '/account',
  '/account/tokens',
  '/settings',
  '/settings/agent',
  '/settings/paths',
  '/agents',
  '/daemons',
  '/operations',
]
for (const width of [320, 390, 768, 1280, 1440]) {
  for (const theme of ['light', 'dark']) {
    test(`layout ${width} ${theme}`, async ({ page, request }, testInfo) => {
      test.setTimeout(240000)
      await page.setViewportSize({ width, height: width < 768 ? 844 : width === 768 ? 1024 : width === 1280 ? 800 : 900 })
      await page.addInitScript((theme) => {
        localStorage.setItem('forge-web-layout', JSON.stringify({ state: { theme }, version: 0 }))
      }, theme)
      const projects = await request.get('/api/v1/projects')
      expect(projects.ok()).toBe(true)
      let project = (await projects.json()).items[0]
      if (!project) {
        const response = await request.post('/api/v1/projects', {
          data: { name: 'Layout audit ' + 'Long title '.repeat(18) },
        })
        expect(response.ok()).toBe(true)
        project = await response.json()
        const task = await request.post(`/api/v1/projects/${project.id}/tasks`, {
          data: { title: 'Long task '.repeat(20) },
        })
        expect(task.ok()).toBe(true)
      }
      const projectRoutes = [
        'board',
        'tasks',
        'settings',
        'settings/repos',
        'settings/members',
        'settings/workers',
        'settings/mcp',
        'settings/hooks',
        'settings/danger',
      ].map((suffix) => `/projects/${project.id}/${suffix}`)
      for (const route of [...routes, ...projectRoutes]) {
        await page.goto(route)
        await expect(page.locator('#main-content')).toBeVisible()
        await expect
          .poll(
            async () => {
              const text = await page.locator('#main-content').innerText()
              return text.trim().length > 20 && !/^Loading[.…]*$/.test(text.trim())
            },
            { timeout: 30000 },
          )
          .toBe(true)
        if (route === '/account') {
          const field = page.getByLabel('Display name', { exact: true })
          await expect(field).toBeVisible()
          const box = await field.boundingBox()
          expect(box!.width).toBeGreaterThan(120)
          expect(box!.x + box!.width).toBeLessThanOrEqual(width)
        }
        await expect(page.locator('html')).toHaveClass(theme === 'dark' ? /dark/ : /^(?!.*dark)/)
        await page.screenshot({
          path: testInfo.outputPath(`${route.replaceAll('/', '-')}.png`),
          fullPage: true,
        })
        const overflow = await page.evaluate(() => {
          const main = document.querySelector('#main-content')!
          return {
            page: document.documentElement.scrollWidth - innerWidth,
            main: main.scrollWidth - main.clientWidth,
          }
        })
        expect.soft(overflow, route).toEqual({ page: 0, main: 0 })
      }
    })
  }
}
