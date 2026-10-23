import { defineConfig, devices } from '@playwright/test'

const webBaseURL = process.env.FORGE_E2E_WEB_BASE_URL ?? 'http://localhost:5173'
const webServerPort = process.env.FORGE_E2E_WEB_PORT

export default defineConfig({
  testDir: './e2e',
  outputDir: 'test-results',
  globalSetup: './e2e/global-setup.ts',
  retries: process.env.CI ? 1 : 0,
  // Integration specs share one Forge API server and SQLite database.
  workers: 1,
  reporter: [['html'], ['list']],
  use: {
    baseURL: webBaseURL,
  },
  webServer: {
    command: 'pnpm run dev',
    env: {
      VITE_DISABLE_REACT_DEVTOOLS: '1',
      VITE_API_PROXY_TARGET:
        process.env.VITE_API_PROXY_TARGET ??
        process.env.FORGE_E2E_BACKEND_BASE_URL ??
        'http://localhost:8080',
    },
    url: webBaseURL,
    reuseExistingServer: webServerPort === undefined,
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
