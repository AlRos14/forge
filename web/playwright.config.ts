import { defineConfig, devices } from '@playwright/test'

export default defineConfig({
  testDir: './e2e',
  outputDir: 'test-results',
  globalSetup: './e2e/global-setup.ts',
  retries: process.env.CI ? 1 : 0,
  // Integration specs share one Forge API server and SQLite database.
  workers: 1,
  reporter: [['html'], ['list']],
  use: {
    baseURL: 'http://localhost:5173',
  },
  webServer: {
    command: 'pnpm run dev',
    env: { VITE_DISABLE_REACT_DEVTOOLS: '1' },
    url: 'http://localhost:5173',
    reuseExistingServer: true,
    timeout: 120000,
  },
  projects: [
    {
      name: 'chromium',
      // Locally, use the installed Google Chrome so devs don't need
      // `playwright install`; CI's playwright container only ships the
      // bundled Chromium, so no channel there.
      use: {
        ...devices['Desktop Chrome'],
        ...(process.env.CI ? {} : { channel: 'chrome' }),
      },
    },
  ],
})
