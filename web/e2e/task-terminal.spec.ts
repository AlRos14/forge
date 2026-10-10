import { expect, test, type Page } from './fixtures'

declare global {
  interface Window {
    __terminalWsSent?: string[]
    __terminalWsUrls?: string[]
    __terminalEmitOutput?: (text: string) => void
  }
}

const PROJECT_ID = 'proj-terminal-e2e'
const TASK_ID = 'task-terminal-e2e'
const SESSION_ID = 'term-terminal-e2e'
const WORKSPACE_ID = 'ws-terminal-e2e'

function mockProject() {
  return {
    id: PROJECT_ID,
    name: 'Terminal E2E Project',
    project_hooks: [],
    primary_repo_id: 'repo-terminal-e2e',
    owner_id: 'e2e-user',
    created_at: '2026-05-20T00:00:00Z',
    updated_at: '2026-05-20T00:00:00Z',
    paused_at: null,
    paused: false,
    version: 1,
  }
}

function mockTask() {
  return {
    id: TASK_ID,
    project_id: PROJECT_ID,
    repo_id: 'repo-terminal-e2e',
    parent_task_id: null,
    title: 'Terminal E2E task',
    description: 'Exercise the workspace terminal.',
    task_type: 'task',
    lifecycle: {
      task_id: TASK_ID,
      state: 'active',
      version: 2,
      reason_kind: null,
      reason_ref: null,
      created_at: '2026-05-20T00:00:00Z',
      updated_at: '2026-05-20T00:00:00Z',
    },
    priority: 50,
    board_position: 10,
    subtask_order: null,
    task_roles: [],
    external_issue_number: null,
    external_issue_url: null,
    workspace: {
      id: WORKSPACE_ID,
      task_id: TASK_ID,
      repo_id: 'repo-terminal-e2e',
      worktree_path: '/tmp/forge-terminal-e2e/repo',
      branch: 'forge/task-terminal-e2e',
      status: 'ready',
      before_sha: null,
      error: null,
      created_at: '2026-05-20T00:00:00Z',
      updated_at: '2026-05-20T00:00:00Z',
    },
    execution_observability: {
      execution_count: 0,
      total_runtime_seconds: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      total_cache_read_tokens: 0,
      total_cache_write_tokens: 0,
      total_tokens: 0,
      total_cost_usd: null,
    },
    version: 1,
    created_at: '2026-05-20T00:00:00Z',
    updated_at: '2026-05-20T00:00:00Z',
  }
}

function mockSession(userId: string, status: 'running' | 'terminated' = 'running') {
  return {
    id: SESSION_ID,
    task_id: TASK_ID,
    workspace_id: WORKSPACE_ID,
    daemon_id: null,
    status,
    rows: 24,
    cols: 80,
    exit_code: null,
    exit_signal: null,
    exit_reason: status === 'terminated' ? 'terminated from web terminal' : null,
    created_at: '2026-05-20T00:00:00Z',
    started_at: '2026-05-20T00:00:01Z',
    last_activity_at: '2026-05-20T00:00:01Z',
    ended_at: status === 'terminated' ? '2026-05-20T00:00:10Z' : null,
    created_by_user_id: userId,
  }
}

async function installMockTerminalWebSocket(page: Page) {
  await page.addInitScript(() => {
    const win = window as Window & {
      __terminalSocket?: { receive: (frame: unknown) => void; close: () => void }
      __terminalWsSent: string[]
      __terminalWsUrls: string[]
      __terminalEmitOutput: (text: string) => void
    }

    class MockWebSocket extends EventTarget {
      static CONNECTING = 0
      static OPEN = 1
      static CLOSING = 2
      static CLOSED = 3

      binaryType: BinaryType = 'blob'
      bufferedAmount = 0
      extensions = ''
      onclose: ((event: CloseEvent) => void) | null = null
      onerror: ((event: Event) => void) | null = null
      onmessage: ((event: MessageEvent) => void) | null = null
      onopen: ((event: Event) => void) | null = null
      protocol = ''
      readyState = MockWebSocket.CONNECTING
      url: string

      constructor(url: string | URL) {
        super()
        this.url = String(url)
        win.__terminalWsUrls.push(this.url)
        win.__terminalSocket = this
        window.setTimeout(() => {
          this.readyState = MockWebSocket.OPEN
          const event = new Event('open')
          this.onopen?.(event)
          this.dispatchEvent(event)
        }, 0)
      }

      send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
        win.__terminalWsSent.push(String(data))
      }

      close() {
        this.readyState = MockWebSocket.CLOSED
        const event = new CloseEvent('close')
        this.onclose?.(event)
        this.dispatchEvent(event)
      }

      receive(frame: unknown) {
        const event = new MessageEvent('message', { data: JSON.stringify(frame) })
        this.onmessage?.(event)
        this.dispatchEvent(event)
      }
    }

    win.__terminalWsSent = []
    win.__terminalWsUrls = []
    win.__terminalEmitOutput = (text: string) => {
      win.__terminalSocket?.receive({ type: 'output', data: btoa(text) })
    }
    win.WebSocket = MockWebSocket as unknown as typeof WebSocket
  })
}

async function setupMockRoutes(page: Page, userId: string) {
  let session = mockSession(userId)
  let sessionCreated = false
  let terminateReason: string | null = null

  await page.route('**/api/v1/projects**', (route) => {
    if (new URL(route.request().url()).pathname !== '/api/v1/projects') {
      return route.fallback()
    }
    return route.fulfill({ json: { items: [mockProject()], has_more: false } })
  })
  await page.route(`**/api/v1/projects/${PROJECT_ID}`, (route) => {
    const url = route.request().url()
    if (url.includes('/tasks') || url.includes('/repos')) {
      return route.fallback()
    }
    return route.fulfill({ json: mockProject() })
  })
  await page.route(`**/api/v1/projects/${PROJECT_ID}/repos*`, (route) =>
    route.fulfill({ json: { items: [], has_more: false } }),
  )
  await page.route(`**/api/v1/projects/${PROJECT_ID}/agents`, (route) =>
    route.fulfill({ json: [] }),
  )
  await page.route(`**/api/v1/projects/${PROJECT_ID}/members`, (route) =>
    route.fulfill({ json: [] }),
  )
  await page.route(`**/api/v1/projects/${PROJECT_ID}/tasks*`, (route) =>
    route.fulfill({ json: { items: [mockTask()], has_more: false } }),
  )
  await page.route(`**/api/v1/tasks/${TASK_ID}`, (route) => {
    const url = route.request().url()
    if (
      url.includes('/executions') ||
      url.includes('/reviews') ||
      url.includes('/validations') ||
      url.includes('/gates') ||
      url.includes('/lifecycle/transitions') ||
      url.includes('/terminals') ||
      url.includes('/workspace')
    ) {
      return route.fallback()
    }
    return route.fulfill({ json: mockTask() })
  })
  await page.route(`**/api/v1/tasks/${TASK_ID}/executions*`, (route) =>
    route.fulfill({ json: { items: [], has_more: false } }),
  )
  await page.route(`**/api/v1/tasks/${TASK_ID}/reviews*`, (route) => route.fulfill({ json: [] }))
  await page.route(`**/api/v1/tasks/${TASK_ID}/validations*`, (route) =>
    route.fulfill({ json: [] }),
  )
  await page.route(`**/api/v1/tasks/${TASK_ID}/gates*`, (route) => route.fulfill({ json: [] }))
  await page.route(`**/api/v1/tasks/${TASK_ID}/lifecycle/transitions*`, (route) =>
    route.fulfill({ json: { items: [], has_more: false } }),
  )
  await page.route(`**/api/v1/tasks/${TASK_ID}/terminals**`, (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (url.pathname.endsWith('/availability')) {
      return route.fulfill({
        json: {
          enabled: true,
          workspace_ready: true,
          daemon_reachable: true,
          active_execution: false,
          session_count_for_task: sessionCreated ? 1 : 0,
          session_count_for_user: sessionCreated ? 1 : 0,
          max_sessions_per_task: 2,
          max_sessions_per_user: 4,
          can_create: true,
          reason: null,
        },
      })
    }
    if (request.method() === 'GET') {
      return route.fulfill({
        json: sessionCreated && session.status === 'running' ? [session] : [],
      })
    }
    if (request.method() === 'POST') {
      sessionCreated = true
      session = mockSession(userId)
      return route.fulfill({
        status: 201,
        json: {
          session,
          attach: {
            attach_token: 'attach-token-start',
            expires_at: '2026-05-20T00:01:00Z',
            ws_url: `/api/v1/terminals/${SESSION_ID}/ws?attach_token=attach-token-start`,
            session_id: SESSION_ID,
          },
        },
      })
    }
    return route.abort()
  })
  await page.route(`**/api/v1/terminals/${SESSION_ID}/attach-token`, (route) =>
    route.fulfill({
      json: {
        attach_token: 'attach-token-reattach',
        expires_at: '2026-05-20T00:01:00Z',
        ws_url: `/api/v1/terminals/${SESSION_ID}/ws?attach_token=attach-token-reattach`,
        session_id: SESSION_ID,
      },
    }),
  )
  await page.route(`**/api/v1/terminals/${SESSION_ID}/resize`, (route) =>
    route.fulfill({ json: session }),
  )
  await page.route(`**/api/v1/terminals/${SESSION_ID}/terminate`, (route) => {
    const body = route.request().postDataJSON() as { reason?: string } | null
    terminateReason = body?.reason ?? null
    session = mockSession(userId, 'terminated')
    return route.fulfill({ json: session })
  })
  await page.route('**/api/v1/agents*', (route) =>
    route.fulfill({ json: { items: [], has_more: false } }),
  )
  await page.route('**/api/v1/notifications**', (route) => {
    if (new URL(route.request().url()).pathname.endsWith('/unread-count')) {
      return route.fulfill({ json: { count: 0 } })
    }
    return route.fulfill({ json: { items: [], has_more: false } })
  })
  await page.route('**/api/v1/events*', (route) =>
    route.fulfill({ status: 200, body: '', contentType: 'text/event-stream' }),
  )

  return {
    terminateReason: () => terminateReason,
  }
}

async function terminalText(page: Page) {
  return (await page.locator('.xterm-rows').first().textContent()) ?? ''
}

async function expectTerminalText(page: Page, text: string) {
  await expect.poll(() => terminalText(page), { timeout: 10_000 }).toContain(text)
}

async function sentTerminalInput(page: Page) {
  return page.evaluate(() => {
    return (window.__terminalWsSent ?? [])
      .map((raw) => {
        try {
          const frame = JSON.parse(raw) as { type?: string; data?: string }
          return frame.type === 'input' && frame.data ? atob(frame.data) : ''
        } catch {
          return ''
        }
      })
      .join('')
  })
}

test('task terminal starts, streams output, accepts input, and terminates', async ({
  page,
  e2eAuth,
}) => {
  await installMockTerminalWebSocket(page)
  const terminalRoutes = await setupMockRoutes(page, e2eAuth.user.id)

  await page.goto(`/tasks/${TASK_ID}`)
  await page.waitForLoadState('domcontentloaded')

  const terminalLink = page.getByRole('link', { name: 'Terminal' })
  await expect(terminalLink).toBeVisible({ timeout: 10_000 })
  await terminalLink.click()
  await expect(page).toHaveURL(/\/tasks\/task-terminal-e2e\/terminal$/)
  await expect(page.getByRole('button', { name: 'Start new session' })).toBeVisible()

  await page.getByRole('button', { name: 'Start new session' }).click()
  await expect(page.getByText('Connected', { exact: true })).toBeVisible({ timeout: 10_000 })
  await expect
    .poll(() => page.evaluate(() => window.__terminalWsUrls ?? []))
    .toContain(
      `ws://localhost:5173/api/v1/terminals/${SESSION_ID}/ws?attach_token=attach-token-start`,
    )

  await page.evaluate(() => window.__terminalEmitOutput?.('forge-terminal-ready\n'))
  await expectTerminalText(page, 'forge-terminal-ready')

  await page.locator('.xterm').click()
  await page.keyboard.type('echo terminal-browser-e2e')
  await page.keyboard.press('Enter')
  await expect
    .poll(() => sentTerminalInput(page), { timeout: 10_000 })
    .toContain('echo terminal-browser-e2e')

  await page.evaluate(() => window.__terminalEmitOutput?.('terminal-browser-e2e\n'))
  await expectTerminalText(page, 'terminal-browser-e2e')

  await page.getByRole('button', { name: 'Terminate' }).click()
  await expect(page.getByText('Terminated', { exact: true })).toBeVisible({ timeout: 10_000 })
  await expectTerminalText(page, '[terminal terminated]')
  expect(terminalRoutes.terminateReason()).toBe('terminated from web terminal')
})
