import { test, expect } from '@playwright/test'
for (const width of [320, 390, 768, 1280, 1440]) {
  for (const theme of ['light', 'dark']) {
    test(`controls ${width} ${theme}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 844 })
      await page.route('**/ui-evidence', (route) =>
        route.fulfill({
          contentType: 'text/html',
          body: `<html class="${theme}"><body><div id="root"></div><script type="module">import RefreshRuntime from '/@react-refresh'; RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => (type) => type; window.__vite_plugin_react_preamble_installed__ = true;</script><script type="module" src="/e2e/ui-consistency-fixture.tsx"></script></body></html>`,
        }),
      )
      await page.goto('/ui-evidence')
      const name = page.getByLabel('Name', { exact: true })
      await expect(name).toBeVisible()
      await page.screenshot({ path: info.outputPath('controls.png') })
      await page.getByRole('button', { name: 'Role', exact: true }).click()
      const box = await page.getByRole('listbox').boundingBox()
      expect(box!.x + box!.width).toBeLessThanOrEqual(width)
      await page.screenshot({ path: info.outputPath('select-open.png') })
      await page.getByRole('option').press('Enter')
      await expect(page.getByRole('button', { name: 'Role', exact: true })).toBeFocused()
      await page.getByRole('button', { name: 'Open dialog' }).click()
      await expect(page.getByRole('dialog')).toBeVisible()
      await page.getByRole('button', { name: 'Close', exact: true }).scrollIntoViewIfNeeded()
      await page.screenshot({ path: info.outputPath('dialog.png') })
      await page.keyboard.press('Escape')
      await expect(page.getByRole('button', { name: 'Open dialog' })).toBeFocused()
      expect(await name.evaluate((el) => el.getBoundingClientRect().height)).toBe(40)
    })
  }
}
