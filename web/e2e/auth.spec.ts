/**
 * Auth E2E — covers task 6.4 (manual E2E for user auth).
 *
 * Requires the backend to be running on :8080 (`make dev`).
 * The Vite dev server on :5173 proxies /api → :8080 automatically.
 */
import { expect, test } from '@playwright/test'

const PASSWORD = 'Password123!'

function uniqueEmail(prefix = 'auth'): string {
  return `e2e-${prefix}-${Date.now()}@test.forge`
}

// ── helpers ──────────────────────────────────────────────────────────────

async function backendAvailable(
  request: Parameters<Parameters<typeof test>[1]>[0]['request'],
): Promise<boolean> {
  try {
    const r = await request.get('/healthz')
    return r.ok()
  } catch {
    return false
  }
}

async function apiRegister(
  request: Parameters<Parameters<typeof test>[1]>[0]['request'],
  email: string,
): Promise<void> {
  const r = await request.post('/api/v1/auth/register', {
    data: { email, password: PASSWORD },
  })
  expect(r.ok(), `register failed: ${await r.text()}`).toBeTruthy()
}

// ── tests ─────────────────────────────────────────────────────────────────

test('unauthenticated visit to / redirects to /login', async ({ page, request }) => {
  if (!(await backendAvailable(request))) {
    test.skip(true, 'backend not running — start with `make dev`')
    return
  }

  await page.goto('/')
  await expect(page).toHaveURL(/\/login/, { timeout: 10_000 })
  await expect(page.getByRole('heading', { name: 'Sign in to Forge' })).toBeVisible()
})

test('register via UI → app shell shown → SSE connected → logout → redirect', async ({
  page,
  request,
}) => {
  if (!(await backendAvailable(request))) {
    test.skip(true, 'backend not running — start with `make dev`')
    return
  }

  const email = uniqueEmail('register')

  // ── 1. Register ──────────────────────────────────────────────────────
  await page.goto('/register')
  await expect(page.getByRole('heading', { name: 'Create your account' })).toBeVisible()

  await page.getByLabel('Email').fill(email)
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
  await page.getByLabel('Confirm password').fill(PASSWORD)

  // Set up SSE request watcher BEFORE clicking submit so we don't miss it
  const sseRequestPromise = page.waitForRequest(/\/api\/v1\/events\?token=/, { timeout: 15_000 })

  await page.getByRole('button', { name: 'Create account' }).click()

  // ── 2. After register: not on /login or /register ────────────────────
  await expect(page).not.toHaveURL(/\/(login|register)/, { timeout: 15_000 })

  // ── 3. App shell is visible (user menu button in header) ─────────────
  await expect(page.getByRole('button', { name: 'User menu' })).toBeVisible({ timeout: 10_000 })

  // ── 4. SSE connection established with token query param ─────────────
  const sseReq = await sseRequestPromise
  expect(sseReq.url()).toContain('/api/v1/events')
  expect(sseReq.url()).toContain('token=')

  // ── 5. Logout via user menu ──────────────────────────────────────────
  await page.getByRole('button', { name: 'User menu' }).click()
  await expect(page.getByText('Sign out')).toBeVisible()
  await page.getByText('Sign out').click()

  // ── 6. Redirect to /login ────────────────────────────────────────────
  await expect(page).toHaveURL(/\/login/, { timeout: 10_000 })
  await expect(page.getByRole('heading', { name: 'Sign in to Forge' })).toBeVisible()

  // ── 7. Protected route redirects back to /login with ?redirect= ──────
  await page.goto('/agents')
  await expect(page).toHaveURL(/\/login/, { timeout: 10_000 })
})

test('login via UI with valid credentials works', async ({ page, request }) => {
  if (!(await backendAvailable(request))) {
    test.skip(true, 'backend not running — start with `make dev`')
    return
  }

  const email = uniqueEmail('login')
  await apiRegister(request, email)

  await page.goto('/login')
  await expect(page.getByRole('heading', { name: 'Sign in to Forge' })).toBeVisible()

  await page.getByLabel('Email').fill(email)
  await page.getByLabel('Password').fill(PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()

  await expect(page).not.toHaveURL(/\/login/, { timeout: 15_000 })
  await expect(page.getByRole('button', { name: 'User menu' })).toBeVisible({ timeout: 10_000 })
})

test('login with wrong password shows error', async ({ page, request }) => {
  if (!(await backendAvailable(request))) {
    test.skip(true, 'backend not running — start with `make dev`')
    return
  }

  const email = uniqueEmail('badpw')
  await apiRegister(request, email)

  await page.goto('/login')
  await page.getByLabel('Email').fill(email)
  await page.getByLabel('Password').fill('wrongpassword')
  await page.getByRole('button', { name: 'Sign in' }).click()

  // Stays on /login, shows error message
  await expect(page).toHaveURL(/\/login/, { timeout: 5_000 })
  await expect(page.getByText(/Invalid|invalid|credentials|failed/i)).toBeVisible({
    timeout: 5_000,
  })
})

test('/login?redirect= preserves the intended destination after auth', async ({
  page,
  request,
}) => {
  if (!(await backendAvailable(request))) {
    test.skip(true, 'backend not running — start with `make dev`')
    return
  }

  const email = uniqueEmail('redirect')
  await apiRegister(request, email)

  // Navigate to a protected page while logged out — should land on /login?redirect=/agents
  await page.goto('/agents')
  await expect(page).toHaveURL(/\/login/, { timeout: 10_000 })

  await page.getByLabel('Email').fill(email)
  await page.getByLabel('Password').fill(PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()

  // After login, should land on /agents (the original destination)
  await expect(page).toHaveURL(/\/agents/, { timeout: 15_000 })
})
