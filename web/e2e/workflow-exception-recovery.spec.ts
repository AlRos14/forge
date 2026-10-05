import { execFile } from 'node:child_process'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'
import { expect, test, type APIRequestContext, type APIResponse, type Page } from './fixtures'

const execFileAsync = promisify(execFile)
const PROJECT_ID = 'proj-exception-recovery'
const TASK_ID = 'task-exception-recovery'

type PaginatedResponse<T> = {
  items: T[]
  has_more: boolean
}

type ProjectResponse = {
  id: string
  name: string
  settings: Record<string, unknown>
}

type RepoResponse = {
  id: string
  project_id: string
}

type TaskResponse = {
  id: string
  project_id: string
  repo_id: string
  title: string
  status: string
  version: number
  remaining_retries: Record<string, number>
  error_annotation?: Record<string, unknown> | null
  blocked?: { reason: string; kind?: string | null } | null
  workflow_health?: {
    kind: string
    label: string
    severity: string
    message?: string | null
  } | null
  workflow_exception?: {
    type: string
    message: string
    review_id?: string | null
    failing_step?: {
      index: number
      command?: string | null
      exit_code?: number | null
      stderr_tail?: string | null
      output_tail?: string | null
    } | null
    actions: Array<{
      kind: string
      label: string
      enabled: boolean
      disabled_reason?: string | null
      requires_reason: boolean
      requires_guidance: boolean
    }>
  } | null
  role_assignments: Array<{
    role_name: string
    assignee_type: string | null
    assignee_id: string | null
  }>
}

async function api<T>(
  request: APIRequestContext,
  method: 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE',
  path: string,
  data?: unknown,
): Promise<T> {
  const response = await request.fetch(path, { method, data, failOnStatusCode: false })
  await expectOk(response, `${method} ${path}`)
  if (response.status() === 204) return undefined as T
  return (await response.json()) as T
}

async function expectOk(response: APIResponse, label: string) {
  if (response.ok()) return
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`)
}

async function poll<T>(
  label: string,
  fn: () => Promise<T | null | undefined | false>,
  options: { timeoutMs: number; intervalMs?: number },
): Promise<T> {
  const startedAt = Date.now()
  const intervalMs = options.intervalMs ?? 5000
  let lastError: unknown

  while (Date.now() - startedAt < options.timeoutMs) {
    try {
      const value = await fn()
      if (value) return value
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs))
  }

  const suffix = lastError instanceof Error ? ` Last error: ${lastError.message}` : ''
  throw new Error(`Timed out waiting for ${label}.${suffix}`)
}

async function getTask(request: APIRequestContext, taskId: string): Promise<TaskResponse> {
  return api<TaskResponse>(request, 'GET', `/api/v1/tasks/${taskId}`)
}

async function waitForTaskStatus(
  request: APIRequestContext,
  taskId: string,
  statuses: string[],
  timeoutMs: number,
): Promise<TaskResponse> {
  return poll(
    `task ${taskId} to reach ${statuses.join(' or ')}`,
    async () => {
      const task = await getTask(request, taskId)
      return statuses.includes(task.status) ? task : null
    },
    { timeoutMs },
  )
}

async function waitForNoRunningExecutions(
  request: APIRequestContext,
  taskId: string,
  timeoutMs: number,
): Promise<boolean> {
  return poll(
    `executions for task ${taskId} to stop`,
    async () => {
      const response = await api<PaginatedResponse<{ id: string; status: string }>>(
        request,
        'GET',
        `/api/v1/tasks/${taskId}/executions`,
      )
      return response.items.every((execution) => execution.status !== 'running') ? true : null
    },
    { timeoutMs, intervalMs: 2000 },
  )
}

async function cleanupTask(request: APIRequestContext, taskId: string): Promise<boolean> {
  const taskResponse = await request.get(`/api/v1/tasks/${taskId}`, { failOnStatusCode: false })
  if (!taskResponse.ok()) return true

  const task = (await taskResponse.json()) as TaskResponse
  if (!['done', 'cancelled'].includes(task.status)) {
    await request.post(`/api/v1/tasks/${taskId}/cancel`, { failOnStatusCode: false })
    const cancelled = await waitForTaskStatus(request, taskId, ['done', 'cancelled'], 120000)
      .then(() => true)
      .catch(() => false)
    if (!cancelled) return false
  }

  const stopped = await waitForNoRunningExecutions(request, taskId, 120000)
    .then(() => true)
    .catch(() => false)
  if (!stopped) return false

  await request.delete(`/api/v1/tasks/${taskId}`, { failOnStatusCode: false })
  return true
}

function emptyHooks() {
  return { before_exit: [], on_exit: [], before_enter: [], on_enter: [], after_enter: [] }
}

function workflow() {
  return {
    states: [
      {
        name: 'todo',
        kind: 'initial',
        column: 'Todo',
        display_name: 'Todo',
        role: null,
        hooks: emptyHooks(),
        cleanup: null,
        gate_config: null,
        dispatch: null,
        triggers: { accept: { to: 'in_progress', dispatch: null } },
        config: {},
      },
      {
        name: 'in_progress',
        kind: 'active',
        column: 'In Progress',
        display_name: 'In Progress',
        role: 'coder',
        hooks: emptyHooks(),
        cleanup: null,
        gate_config: null,
        dispatch: null,
        triggers: {},
        config: {},
      },
      {
        name: 'review',
        kind: 'gate',
        column: 'Review',
        display_name: 'Review',
        role: 'reviewer',
        hooks: emptyHooks(),
        cleanup: null,
        gate_config: { reject_target: 'in_progress', max_rejections: 2 },
        dispatch: null,
        triggers: {},
        config: {},
      },
      {
        name: 'done',
        kind: 'terminal',
        column: 'Done',
        display_name: 'Done',
        role: null,
        hooks: emptyHooks(),
        cleanup: null,
        gate_config: null,
        dispatch: null,
        triggers: {},
        config: {},
      },
      {
        name: 'cancelled',
        kind: 'terminal',
        column: 'Cancelled',
        display_name: 'Cancelled',
        role: null,
        hooks: emptyHooks(),
        cleanup: null,
        gate_config: null,
        dispatch: null,
        triggers: {},
        config: {},
      },
    ],
    roles: [
      { name: 'coder', display_name: 'Coder', description: '' },
      { name: 'reviewer', display_name: 'Reviewer', description: '' },
    ],
    configuration: [],
    cancellation_state: 'cancelled',
  }
}

function project(settings: Record<string, unknown> = {}) {
  return {
    id: PROJECT_ID,
    name: 'Exception Recovery Project',
    settings,
    workflow_template_name: null,
    default_review_config: { ci_steps: [], review_prompt: null },
    created_at: '2026-05-02T00:00:00Z',
    updated_at: '2026-05-02T00:00:00Z',
  }
}

function taskDefaults() {
  return {
    task_type: 'task' as const,
    description: null,
    parent_task_id: null,
    assignee_type: null,
    assignee_id: null,
    task_roles: [],
    subtask_order: null,
    board_position: 50,
    awaiting_human: false,
    task_state_config: null,
    review_passed_at: null,
    archived_at: null,
    workspace: null,
    plan_progress: null,
    plan_artifact: null,
    execution_actions: [],
    execution_observability: {
      execution_count: 0,
      active_execution_id: null,
      active_role: null,
      active_started_at: null,
      active_elapsed_seconds: null,
      latest_execution_id: null,
      latest_execution_status: null,
      latest_role: null,
      latest_started_at: null,
      latest_stopped_at: null,
      latest_runtime_seconds: null,
      total_runtime_seconds: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      total_cache_read_tokens: 0,
      total_cache_write_tokens: 0,
      total_tokens: 0,
      total_cost_usd: null,
    },
    external_issue_number: null,
    external_issue_url: null,
    created_at: '2026-05-02T00:00:00Z',
    updated_at: '2026-05-02T10:00:00Z',
  }
}

function budgetExhaustedTask(): TaskResponse {
  return {
    ...taskDefaults(),
    id: TASK_ID,
    project_id: PROJECT_ID,
    repo_id: 'repo-1',
    title: 'Task with exhausted review budget',
    status: 'review',
    version: 5,
    priority: 50,
    remaining_retries: { review: 0 },
    role_assignments: [
      { role_name: 'coder', assignee_type: 'agent', assignee_id: 'agent-1' },
      { role_name: 'reviewer', assignee_type: 'agent', assignee_id: 'agent-2' },
    ],
    error_annotation: {
      type: 'review_budget_exhausted',
      annotation_type: 'review_budget_exhausted',
      blocking_reason: 'review retry budget exhausted',
      blocked_by: 'system:review',
      blocked_at: '2026-05-02T10:00:00Z',
      blocked_execution_id: 'exec-reviewer-1',
      artifact: null,
      message: 'Review retry budget exhausted after 2 attempts',
      hook: null,
      recovery_actions: ['cancel_task'],
    },
    blocked: {
      reason: 'review retry budget exhausted',
      kind: 'review_gate_failed',
    },
    workflow_health: {
      kind: 'blocked',
      label: 'Blocked',
      severity: 'error',
      message: 'Review retry budget exhausted after 2 attempts',
    },
    workflow_exception: {
      type: 'retry_budget_exhausted',
      message: 'Review retry budget exhausted after 2 attempts',
      review_id: 'review-failed-1',
      execution_id: 'exec-reviewer-1',
      state: 'review',
      role: 'reviewer',
      target_state: 'in_progress',
      target_role: 'coder',
      failing_step: {
        index: 0,
        command: 'npm run build',
        exit_code: 1,
        stderr_tail: 'Error: Cannot find module ./missing\n',
        output_tail: 'Build failed with 1 error\n',
      },
      related_evidence: [],
      actions: [
        {
          kind: 'cancel_task',
          label: 'Cancel Task',
          enabled: true,
          disabled_reason: null,
          requires_reason: false,
          requires_guidance: false,
          propagates: false,
          target_state: null,
          target_role: null,
          target_execution_id: null,
        },
      ],
    },
  } as TaskResponse
}

function reviewFailedTask(): TaskResponse {
  return {
    ...taskDefaults(),
    id: TASK_ID,
    project_id: PROJECT_ID,
    repo_id: 'repo-1',
    title: 'Task with failed review CI',
    status: 'review',
    version: 3,
    priority: 50,
    remaining_retries: { review: 1 },
    role_assignments: [
      { role_name: 'coder', assignee_type: 'agent', assignee_id: 'agent-1' },
    ],
    error_annotation: null,
    blocked: null,
    workflow_health: {
      kind: 'idle',
      label: 'Idle',
      severity: 'info',
      message: 'Review CI failed: npm run build (exit 1)',
    },
    workflow_exception: {
      type: 'review_failed',
      message: 'Review CI failed: npm run build (exit 1)',
      review_id: 'review-1',
      execution_id: null,
      state: 'review',
      role: 'reviewer',
      target_state: 'in_progress',
      target_role: 'coder',
      failing_step: {
        index: 0,
        command: 'npm run build',
        exit_code: 1,
        stderr_tail: 'Error: Cannot find module ./App\n',
        output_tail: 'vite build failed\n',
      },
      related_evidence: [],
      actions: [
        {
          kind: 'retry_hook',
          label: 'Retry Review',
          enabled: false,
          disabled_reason: 'No blocking annotation to retry',
          requires_reason: false,
          requires_guidance: false,
          propagates: false,
          target_state: null,
          target_role: null,
          target_execution_id: null,
        },
        {
          kind: 'open_interactive',
          label: 'Open Interactive Session',
          enabled: true,
          disabled_reason: null,
          requires_reason: false,
          requires_guidance: false,
          propagates: false,
          target_state: null,
          target_role: null,
          target_execution_id: null,
        },
        {
          kind: 'cancel_task',
          label: 'Cancel Task',
          enabled: true,
          disabled_reason: null,
          requires_reason: false,
          requires_guidance: false,
          propagates: false,
          target_state: null,
          target_role: null,
          target_execution_id: null,
        },
      ],
    },
  } as TaskResponse
}

async function mockExceptionTaskRoutes(page: Page, task: TaskResponse) {
  let recoverBody: Record<string, unknown> | null = null

  await Promise.all([
    page.route('**/api/v1/projects', (route) =>
      route.fulfill({ json: { items: [project()], has_more: false } }),
    ),
    page.route(`**/api/v1/projects/${PROJECT_ID}`, (route) =>
      route.fulfill({ json: project() }),
    ),
    page.route(`**/api/v1/projects/${PROJECT_ID}/workflow`, (route) =>
      route.fulfill({ json: workflow() }),
    ),
    page.route(`**/api/v1/projects/${PROJECT_ID}/tasks*`, (route) =>
      route.fulfill({ json: { items: [task], has_more: false } }),
    ),
    page.route(`**/api/v1/tasks/${TASK_ID}`, async (route) => {
      if (route.request().method() === 'GET') {
        return route.fulfill({ json: task })
      }
      return route.fulfill({ json: task })
    }),
    page.route(`**/api/v1/tasks/${TASK_ID}/recover`, async (route) => {
      recoverBody = (await route.request().postDataJSON()) as Record<string, unknown>
      return route.fulfill({ json: task })
    }),
    page.route(`**/api/v1/tasks/${TASK_ID}/executions*`, (route) =>
      route.fulfill({ json: { items: [], has_more: false } }),
    ),
    page.route(`**/api/v1/tasks/${TASK_ID}/reviews*`, (route) =>
      route.fulfill({ json: [] }),
    ),
    page.route(`**/api/v1/tasks/${TASK_ID}/transitions*`, (route) =>
      route.fulfill({ json: [] }),
    ),
    page.route(`**/api/v1/tasks/${TASK_ID}/comments*`, (route) =>
      route.fulfill({ json: { items: [], has_more: false } }),
    ),
    page.route(`**/api/v1/tasks/${TASK_ID}/diff`, (route) =>
      route.fulfill({ status: 400, body: 'workspace.not_found' }),
    ),
    page.route('**/api/v1/agents*', (route) =>
      route.fulfill({ json: { items: [], has_more: false } }),
    ),
    page.route('**/api/v1/notifications/unread-count*', (route) =>
      route.fulfill({ json: { count: 0 } }),
    ),
    page.route('**/api/v1/notifications*', (route) =>
      route.fulfill({ json: { items: [], has_more: false } }),
    ),
    page.route('**/api/v1/events*', (route) =>
      route.fulfill({ status: 200, body: '', contentType: 'text/event-stream' }),
    ),
  ])

  return () => recoverBody
}

test.describe('workflow exception diagnostics (mock)', () => {
  test('exhausted exception waits for an external decision without retry actions', async ({ page }) => {
    const task = budgetExhaustedTask()
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    await expect(
      page.getByText('Review retry budget exhausted', { exact: false }).first(),
    ).toBeVisible({ timeout: 15000 })

    await expect(page.getByText('npm run build')).toBeVisible()
    await expect(page.getByText('exit 1', { exact: false }).first()).toBeVisible()
    await expect(page.getByText('Cannot find module ./missing', { exact: false })).toBeVisible()

    await expect(page.getByRole('button', { name: 'Reset Retry Window' })).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Resume Process' })).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Retry Review' })).toHaveCount(0)
    await expect(
      page.getByRole('button', { name: 'Cancel Task', exact: true }).first(),
    ).toBeVisible()
  })

  test('renders review-failed exception with health label', async ({ page }) => {
    const task = reviewFailedTask()
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    await expect(
      page.getByText('Idle').first(),
    ).toBeVisible({ timeout: 15000 })

    await expect(page.getByText('npm run build').first()).toBeVisible()
    await expect(page.getByText('Cannot find module ./App', { exact: false }).first()).toBeVisible()

    await expect(
      page.getByRole('button', { name: 'Cancel Task', exact: true }).first(),
    ).toBeVisible()
  })

  test('exhausted task has no retry reset action to submit', async ({ page }) => {
    const task = budgetExhaustedTask()
    const getRecoverBody = await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    await expect(page.getByRole('button', { name: 'Reset Retry Window' })).toHaveCount(0)
    expect(getRecoverBody()).toBeNull()
  })

  test('exhaustion has no secondary recovery menu', async ({ page }) => {
    const task = budgetExhaustedTask()
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    const dropdownTrigger = page.locator('button[class*="rounded-l-none"]').first()
    await expect(dropdownTrigger).toHaveCount(0)
  })

  test('open-interactive action is visible and clickable', async ({ page }) => {
    const task = reviewFailedTask()
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    const openButton = page.getByRole('button', { name: /open interactive/i }).first()
    await expect(openButton).toBeVisible({ timeout: 15000 })
    await expect(openButton).toBeEnabled()
  })

  test('blocking annotation takes precedence over failed review in exception type', async ({ page }) => {
    const task: TaskResponse = {
      ...budgetExhaustedTask(),
      workflow_exception: {
        ...budgetExhaustedTask().workflow_exception!,
        type: 'retry_budget_exhausted',
        related_evidence: [
          { kind: 'review_failed', id: 'review-also-failed', message: 'CI also failed' },
        ],
      },
    }
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    await expect(
      page.getByText('Retry Budget Exhausted', { exact: false }).first(),
    ).toBeVisible({ timeout: 15000 })

    await expect(
      page.getByText('Review Failed', { exact: false }).first(),
    ).toBeVisible()
  })

  test('running health kind shows execution context', async ({ page }) => {
    const task: TaskResponse = {
      ...taskDefaults(),
      id: TASK_ID,
      project_id: PROJECT_ID,
      repo_id: 'repo-1',
      title: 'Running task',
      status: 'in_progress',
      version: 2,
      priority: 50,
      remaining_retries: {},
      role_assignments: [
        { role_name: 'coder', assignee_type: 'agent', assignee_id: 'agent-1' },
      ],
      error_annotation: null,
      blocked: null,
      workflow_health: {
        kind: 'running',
        label: 'Running',
        severity: 'info',
        message: 'coder execution is running',
      },
      workflow_exception: null,
    } as TaskResponse
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    await expect(page.getByText('Running').first()).toBeVisible({ timeout: 15000 })
  })

  test('waiting-for-agent health kind renders correctly', async ({ page }) => {
    const task: TaskResponse = {
      ...taskDefaults(),
      id: TASK_ID,
      project_id: PROJECT_ID,
      repo_id: 'repo-1',
      title: 'Waiting task',
      status: 'review',
      version: 2,
      priority: 50,
      remaining_retries: {},
      role_assignments: [],
      error_annotation: null,
      blocked: null,
      workflow_health: {
        kind: 'waiting_for_agent',
        label: 'Waiting for Agent',
        severity: 'info',
        message: 'Waiting for reviewer assignment',
      },
      workflow_exception: null,
    } as TaskResponse
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    await expect(page.getByText('Waiting for Agent').first()).toBeVisible({ timeout: 15000 })
  })

  test('failed health kind renders with error severity', async ({ page }) => {
    const task: TaskResponse = {
      ...taskDefaults(),
      id: TASK_ID,
      project_id: PROJECT_ID,
      repo_id: 'repo-1',
      title: 'Failed task',
      status: 'in_progress',
      version: 2,
      priority: 50,
      remaining_retries: {},
      role_assignments: [],
      error_annotation: null,
      blocked: null,
      failed: { reason: 'Execution crashed', created_at: '2026-05-02T10:00:00Z' },
      workflow_health: {
        kind: 'failed',
        label: 'Failed',
        severity: 'error',
        message: 'Execution crashed',
      },
      workflow_exception: null,
    } as TaskResponse
    await mockExceptionTaskRoutes(page, task)

    await page.goto(`/tasks/${TASK_ID}`)

    await expect(page.getByText('Failed').first()).toBeVisible({ timeout: 15000 })
  })

  test('health label is visible on board card', async ({ page }) => {
    const task = budgetExhaustedTask()

    await Promise.all([
      page.route('**/api/v1/projects', (route) =>
        route.fulfill({ json: { items: [project()], has_more: false } }),
      ),
      page.route(`**/api/v1/projects/${PROJECT_ID}`, (route) =>
        route.fulfill({ json: project() }),
      ),
      page.route(`**/api/v1/projects/${PROJECT_ID}/workflow`, (route) =>
        route.fulfill({ json: workflow() }),
      ),
      page.route(`**/api/v1/projects/${PROJECT_ID}/tasks*`, (route) =>
        route.fulfill({ json: { items: [task], has_more: false } }),
      ),
      page.route('**/api/v1/agents*', (route) =>
        route.fulfill({ json: { items: [], has_more: false } }),
      ),
      page.route('**/api/v1/notifications/unread-count*', (route) =>
        route.fulfill({ json: { count: 0 } }),
      ),
      page.route('**/api/v1/notifications*', (route) =>
        route.fulfill({ json: { items: [], has_more: false } }),
      ),
      page.route('**/api/v1/events*', (route) =>
        route.fulfill({ status: 200, body: '', contentType: 'text/event-stream' }),
      ),
    ])

    await page.goto(`/projects/${PROJECT_ID}/board`)
    await page.waitForLoadState('domcontentloaded')

    await expect(
      page.getByText('Blocked', { exact: true }).first(),
    ).toBeVisible({ timeout: 15000 })
  })

  test('health labels render on task list page', async ({ page }) => {
    const tasks = [
      { ...budgetExhaustedTask(), id: 'task-1', title: 'Blocked task' },
      {
        ...taskDefaults(),
        id: 'task-2',
        project_id: PROJECT_ID,
        repo_id: 'repo-1',
        title: 'Running task',
        status: 'in_progress',
        version: 1,
        priority: 50,
        remaining_retries: {},
        role_assignments: [],
        error_annotation: null,
        blocked: null,
        workflow_health: { kind: 'running', label: 'Running', severity: 'info', message: null },
        workflow_exception: null,
      },
    ]

    await Promise.all([
      page.route('**/api/v1/projects', (route) =>
        route.fulfill({ json: { items: [project()], has_more: false } }),
      ),
      page.route(`**/api/v1/projects/${PROJECT_ID}`, (route) =>
        route.fulfill({ json: project() }),
      ),
      page.route(`**/api/v1/projects/${PROJECT_ID}/workflow`, (route) =>
        route.fulfill({ json: workflow() }),
      ),
      page.route(`**/api/v1/projects/${PROJECT_ID}/tasks*`, (route) =>
        route.fulfill({ json: { items: tasks, has_more: false } }),
      ),
      page.route('**/api/v1/agents*', (route) =>
        route.fulfill({ json: { items: [], has_more: false } }),
      ),
      page.route('**/api/v1/notifications/unread-count*', (route) =>
        route.fulfill({ json: { count: 0 } }),
      ),
      page.route('**/api/v1/notifications*', (route) =>
        route.fulfill({ json: { items: [], has_more: false } }),
      ),
      page.route('**/api/v1/events*', (route) =>
        route.fulfill({ status: 200, body: '', contentType: 'text/event-stream' }),
      ),
    ])

    await page.goto(`/projects/${PROJECT_ID}/tasks`)
    await page.waitForLoadState('domcontentloaded')

    await expect(page.getByText('Blocked').first()).toBeVisible({ timeout: 15000 })
    await expect(page.getByText('Running').first()).toBeVisible()
  })
})

async function createGitFixture(runId: string): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), `forge-recovery-${runId}-`))
  await execFileAsync('git', ['init'], { cwd: root })
  await execFileAsync('git', ['checkout', '-B', 'main'], { cwd: root })
  await execFileAsync('git', ['config', 'user.email', 'e2e@test'], { cwd: root })
  await execFileAsync('git', ['config', 'user.name', 'E2E'], { cwd: root })
  await execFileAsync('git', ['commit', '--allow-empty', '-m', 'init'], { cwd: root })
  return root
}

async function createTestProject(
  request: APIRequestContext,
  runId: string,
  fixturePath: string,
): Promise<{ projectId: string }> {
  const project = await api<ProjectResponse>(request, 'POST', '/api/v1/projects', {
    name: `Recovery E2E ${runId}`,
    default_review_config: { ci_steps: [], review_prompt: null },
  })
  await api<RepoResponse>(request, 'POST', `/api/v1/projects/${project.id}/repos`, {
    name: `recovery-repo-${runId}`,
    remote_url: fixturePath,
    local_path: fixturePath,
    work_mode: 'direct_merge',
    default_branch: 'main',
  })
  return { projectId: project.id }
}

test.describe('workflow exception recovery (integration)', () => {
  test('workflow_health present on task detail and list responses', async ({ request }) => {
    test.setTimeout(2 * 60 * 1000)
    const projectsResponse = await request.get('/api/v1/projects', { failOnStatusCode: false })
    await expectOk(projectsResponse, 'GET /api/v1/projects')
    const runId = `health-${Date.now()}`
    const createdTaskIds: string[] = []
    const fixturePath = await createGitFixture(runId)

    try {
      const { projectId } = await createTestProject(request, runId, fixturePath)
      const task = await api<TaskResponse>(
        request, 'POST', `/api/v1/projects/${projectId}/tasks`,
        { title: `Health check ${runId}`, description: 'Verify health field', task_type: 'task', priority: 0 },
      )
      createdTaskIds.push(task.id)

      const detail = await getTask(request, task.id)
      expect(detail.workflow_health).toBeDefined()
      expect(detail.workflow_health?.kind).toBeDefined()
      expect(detail.workflow_health?.severity).toBeDefined()
      expect(detail.workflow_health?.label).toBeDefined()

      const list = await api<PaginatedResponse<TaskResponse>>(request, 'GET', `/api/v1/projects/${projectId}/tasks`)
      const listTask = list.items.find((t) => t.id === task.id)
      expect(listTask?.workflow_health).toBeDefined()
    } finally {
      for (const taskId of createdTaskIds.reverse()) await cleanupTask(request, taskId)
      await rm(fixturePath, { recursive: true, force: true })
    }
  })

  test('retired reset_retry_window wire value is rejected during request decoding', async ({ request }) => {
    test.setTimeout(2 * 60 * 1000)
    const runId = `reset-reject-${Date.now()}`
    const createdTaskIds: string[] = []
    const fixturePath = await createGitFixture(runId)

    try {
      const { projectId } = await createTestProject(request, runId, fixturePath)
      const task = await api<TaskResponse>(
        request, 'POST', `/api/v1/projects/${projectId}/tasks`,
        { title: `Reset reject ${runId}`, description: 'Test', task_type: 'task', priority: 0 },
      )
      createdTaskIds.push(task.id)

      const resp = await request.fetch(`/api/v1/tasks/${task.id}/recover`, {
        method: 'POST',
        data: { action: 'reset_retry_window', reason: 'should fail' },
        failOnStatusCode: false,
      })
      expect(resp.status()).toBe(422)
    } finally {
      for (const taskId of createdTaskIds.reverse()) await cleanupTask(request, taskId)
      await rm(fixturePath, { recursive: true, force: true })
    }
  })


})
