import { expect, test } from './fixtures'

const projectId = 'project-browser'
const taskId = 'task-browser'
const now = '2026-11-05T10:00:00Z'

function project(ownerId: string) {
  return {
    id: projectId,
    name: 'Browser Project',
    project_hooks: [],
    primary_repo_id: null,
    owner_id: ownerId,
    created_at: now,
    updated_at: now,
    paused_at: null,
    paused: false,
    version: 1,
  }
}

function task() {
  return {
    id: taskId,
    project_id: projectId,
    repo_id: null,
    parent_task_id: null,
    title: 'Browser task exact context',
    description: 'Task description for the browser Copilot smoke.',
    lifecycle: {
      task_id: taskId,
      state: 'active',
      version: 1,
      reason_kind: null,
      reason_ref: null,
      created_at: now,
      updated_at: now,
    },
    priority: 0,
    board_position: 0,
    subtask_order: null,
    task_roles: [],
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
    workspace: null,
    external_issue_number: null,
    external_issue_url: null,
    version: 1,
    created_at: now,
    updated_at: now,
  }
}

async function mockProjectCopilotApi(
  page: import('@playwright/test').Page,
  userId: string,
  captureMessage?: (body: Record<string, unknown>) => void,
) {
  const currentProject = project(userId)
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const path = url.pathname.replace('/api/v1', '')
    const method = request.method()
    let body: unknown = []
    let status = 200

    if (path === '/projects' && method === 'GET') {
      body = { items: [currentProject], next_cursor: null, total_count: 1 }
    } else if (path === `/projects/${projectId}` && method === 'GET') {
      body = currentProject
    } else if (path === `/projects/${projectId}/tasks` && method === 'GET') {
      body = { items: [task()], next_cursor: null, total_count: 1 }
    } else if (path === `/projects/${projectId}/members` && method === 'GET') {
      body = [
        {
          id: 'browser-owner-membership',
          user_id: userId,
          email: 'browser-owner@example.test',
          display_name: 'Browser Owner',
          role: 'owner',
          created_at: now,
        },
      ]
    } else if (path === `/tasks/${taskId}` && method === 'GET') {
      body = task()
    } else if (path === `/projects/${projectId}/copilot` && method === 'GET') {
      body = {
        project_id: projectId,
        agent_id: 'claude-agent',
        agent_name: 'Claude Project Agent',
        version: 1,
        active_run: null,
        supported_executor_types: ['claude_code'],
        supported_daemon_id: 'embedded-host',
      }
    } else if (path === `/projects/${projectId}/copilot/messages` && method === 'GET') {
      body = { items: [], next_cursor: null }
    } else if (path === `/projects/${projectId}/copilot/activity` && method === 'GET') {
      body = { items: [], next_cursor: null }
    } else if (path === `/projects/${projectId}/copilot/messages` && method === 'POST') {
      const payload = request.postDataJSON() as Record<string, unknown>
      captureMessage?.(payload)
      const run = {
        id: 'browser-run',
        project_id: projectId,
        agent_id: 'claude-agent',
        binding_version: 1,
        context_task_id: payload.task_id ?? null,
        status: 'queued',
        failure_code: null,
        failure_message: null,
        created_at: now,
        started_at: null,
        completed_at: null,
        updated_at: now,
      }
      body = {
        run,
        user_message: {
          id: 'browser-message',
          run_id: run.id,
          project_id: projectId,
          role: 'user',
          actor_type: 'human',
          actor_id: userId,
          content: payload.content,
          created_at: now,
        },
      }
      status = 202
    } else if (path === `/projects/${projectId}/copilot/runs/browser-run` && method === 'GET') {
      body = {
        run: {
          id: 'browser-run',
          project_id: projectId,
          agent_id: 'claude-agent',
          binding_version: 1,
          context_task_id: taskId,
          status: 'queued',
          failure_code: null,
          failure_message: null,
          created_at: now,
          started_at: null,
          completed_at: null,
          updated_at: now,
        },
        user_message: null,
        assistant_message: null,
      }
    } else if (path === '/events/ticket' && method === 'POST') {
      body = { ticket: 'browser-smoke-ticket', expires_at: now }
    } else if (path.startsWith(`/tasks/${taskId}/`)) {
      body = path.endsWith('/executions') ? { items: [], next_cursor: null } : []
    }

    await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) })
  })
}

test('Project Copilot stays available on a Task route and sends its exact context on desktop', async ({
  page,
  e2eAuth,
}) => {
  let sentTaskId: unknown
  await mockProjectCopilotApi(page, e2eAuth.user.id, (body) => {
    sentTaskId = body.task_id
  })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto(`/tasks/${taskId}`)

  await expect(page.getByRole('heading', { name: 'Browser task exact context' })).toBeVisible()
  await page.getByRole('button', { name: 'Open Project Copilot' }).click()
  const drawer = page.getByRole('dialog', { name: 'Project Copilot panel' })
  await expect(drawer).toBeVisible()
  await expect(drawer).not.toHaveAttribute('aria-modal', 'true')
  await expect(drawer.getByTitle('Browser task exact context')).toBeVisible()

  await drawer.getByRole('textbox', { name: 'Message to Project Copilot' }).fill('Organize this exact task.')
  await drawer.getByRole('button', { name: 'Send message' }).click()
  await expect.poll(() => sentTaskId).toBe(taskId)
})

test('Project Copilot opens as a modal drawer on mobile with the Task attached', async ({
  page,
  e2eAuth,
}) => {
  await mockProjectCopilotApi(page, e2eAuth.user.id)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto(`/tasks/${taskId}`)

  await page.getByRole('button', { name: 'Open Project Copilot' }).click()
  const drawer = page.getByRole('dialog', { name: 'Project Copilot panel' })
  await expect(drawer).toBeVisible()
  await expect(drawer).toHaveAttribute('aria-modal', 'true')
  await expect(drawer.getByTitle('Browser task exact context')).toBeVisible()
})
