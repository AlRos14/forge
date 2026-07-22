import { spawn } from 'node:child_process'
import { createServer } from 'node:net'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { chromium } from '@playwright/test'
import lighthouse from 'lighthouse'
import desktopConfig from 'lighthouse/core/config/desktop-config.js'

const baseURL = process.env.FORGE_AUDIT_BASE_URL ?? 'http://127.0.0.1:8080'
const email = process.env.FORGE_E2E_EMAIL ?? 'e2e-default@test.forge'
const password = process.env.FORGE_E2E_PASSWORD ?? 'Password123!'
const repetitions = Number(process.env.FORGE_AUDIT_REPETITIONS ?? '3')
const chromePath =
  process.env.CHROME_PATH ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
const outputDir = join(process.cwd(), '..', 'test', 'lighthouse')

if (!Number.isInteger(repetitions) || repetitions < 1 || repetitions > 5) {
  throw new Error('FORGE_AUDIT_REPETITIONS must be an integer from 1 to 5')
}

const auth = await requestJson('/api/v1/auth/login', {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ email, password }),
})
const user = await requestJson('/api/v1/auth/me', {
  headers: { authorization: `Bearer ${auth.access_token}` },
})
const projects = await requestJson('/api/v1/projects?limit=1', {
  headers: { authorization: `Bearer ${auth.access_token}` },
})
const projectId = process.env.FORGE_AUDIT_PROJECT_ID ?? projects.items?.[0]?.id
if (!projectId) throw new Error('No project is available for the board audit')

const authStorage = JSON.stringify({
  state: {
    accessToken: auth.access_token,
    refreshToken: auth.refresh_token,
    user,
  },
  version: 0,
})
const boardURL = `${baseURL}/projects/${projectId}/board`
const port = await availablePort()
const profileDir = await mkdtemp(join(tmpdir(), 'forge-lighthouse-'))
const chrome = spawn(
  chromePath,
  [
    '--headless=new',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profileDir}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-background-networking',
  ],
  { stdio: 'ignore' },
)

let browser
try {
  await waitForChrome(port)
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`)
  const context = browser.contexts()[0]
  const page = await context.newPage()
  await page.goto(baseURL)
  await page.evaluate((value) => localStorage.setItem('forge-auth', value), authStorage)
  await page.goto(boardURL)
  await page.locator('[data-board-page]').waitFor({ state: 'visible', timeout: 15_000 })
  await page.close()

  await mkdir(outputDir, { recursive: true })
  const summary = { boardURL, repetitions, desktop: [], mobile: [] }
  for (const mode of ['desktop', 'mobile']) {
    for (let run = 1; run <= repetitions; run += 1) {
      const result = await lighthouse(
        boardURL,
        {
          port,
          logLevel: 'error',
          output: 'json',
          onlyCategories: ['performance', 'accessibility', 'best-practices', 'seo'],
          disableStorageReset: true,
        },
        mode === 'desktop' ? desktopConfig : undefined,
      )
      if (!result) throw new Error(`Lighthouse returned no ${mode} result for run ${run}`)
      const scores = Object.fromEntries(
        Object.entries(result.lhr.categories).map(([category, value]) => [
          category,
          Math.round((value.score ?? 0) * 100),
        ]),
      )
      summary[mode].push(scores)
      await writeFile(join(outputDir, `board-${mode}-${run}.json`), result.report)
    }
  }

  const medians = Object.fromEntries(
    ['desktop', 'mobile'].map((mode) => [
      mode,
      Object.fromEntries(
        ['performance', 'accessibility', 'best-practices', 'seo'].map((category) => [
          category,
          median(summary[mode].map((scores) => scores[category])),
        ]),
      ),
    ]),
  )
  await writeFile(
    join(outputDir, 'board-summary.json'),
    `${JSON.stringify({ ...summary, medians }, null, 2)}\n`,
  )
  process.stdout.write(`${JSON.stringify(medians)}\n`)
} finally {
  await browser?.close().catch(() => undefined)
  chrome.kill('SIGTERM')
  await waitForChromeExit(chrome)
  await rm(profileDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
}

async function requestJson(path, init) {
  const response = await fetch(`${baseURL}${path}`, init)
  if (!response.ok) {
    throw new Error(`${path} failed with HTTP ${response.status}`)
  }
  return response.json()
}

async function availablePort() {
  const server = createServer()
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const address = server.address()
  const port = typeof address === 'object' && address ? address.port : undefined
  await new Promise((resolve) => server.close(resolve))
  if (!port) throw new Error('Could not allocate a Chrome debugging port')
  return port
}

async function waitForChrome(port) {
  const endpoint = `http://127.0.0.1:${port}/json/version`
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const response = await fetch(endpoint)
      if (response.ok) return
    } catch {
      // Chrome is still starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error('Chrome remote debugging endpoint did not become ready')
}

async function waitForChromeExit(process) {
  if (process.exitCode !== null) return
  await Promise.race([
    new Promise((resolve) => process.once('exit', resolve)),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ])
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right)
  return sorted[Math.floor(sorted.length / 2)]
}
