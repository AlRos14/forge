import { test, expect } from './fixtures'
test('profile error, recovery, keyboard navigation and save', async ({ page }, info) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/account')
  const name = page.getByLabel('Display name', { exact: true })
  await name.fill('Layout audit user')
  await page.route('**/api/v1/auth/me', async (route) => {
    if (route.request().method() === 'PATCH')
      await route.fulfill({
        status: 409,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Conflict: this record changed. Reload and try again.' }),
      })
    else await route.continue()
  })
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(page.getByText(/Conflict: this record changed/)).toBeVisible()
  await page.screenshot({ path: info.outputPath('profile-conflict.png') })
  await page.unroute('**/api/v1/auth/me')
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(page.getByText('Profile updated')).toBeVisible()
  await page.getByRole('link', { name: 'Access Tokens', exact: true }).focus()
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(/account\/tokens/)
})
