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
    url: 'http://localhost:5173',
    reuseExistingServer: true,
    timeout: 120000,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
})
