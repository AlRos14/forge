import { expect, test } from './fixtures'

test('loads the app shell', async ({ page }) => {
  await page.goto('/')
  await page.waitForLoadState('domcontentloaded')

  await expect.poll(() => page.title(), { timeout: 10000 }).not.toBe('')
  await expect(page.locator('body')).toBeVisible()
})
