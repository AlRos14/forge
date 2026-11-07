import { test, expect } from '@playwright/test'
for (const width of [320, 390, 768, 1280, 1440]) {
  for (const theme of ['light', 'dark']) {
    test(`authentication ${width} ${theme}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 844 })
      await page.addInitScript(
        (theme) =>
          localStorage.setItem(
            'forge-web-layout',
            JSON.stringify({ state: { theme }, version: 0 }),
          ),
        theme,
      )
      for (const route of ['/login', '/register']) {
        await page.goto(route)
        await expect(page.getByLabel('Email', { exact: true })).toBeVisible()
        await page.screenshot({ path: info.outputPath(`${route.slice(1)}.png`) })
        expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBe(0)
      }
    })
  }
}
