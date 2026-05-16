import { resolve } from 'node:path'
import { expect, test, type APIRequestContext, type APIResponse } from './fixtures'

type PaginatedResponse<T> = {
  items: T[]
  has_more: boolean
}

type Agent = { id: string }
type Project = { id: string }
type Repo = { id: string }
type Task = { id: string }
type Conversation = { id: string }

type ConversationMessage = {
  role: 'user' | 'assistant' | 'system'
  status: 'complete' | 'streaming' | 'failed' | 'cancelled'
  content: string
  error: string | null
}

type Execution = {
  id: string
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  summary: string | null
  error: string | null
}

const shouldRun = process.env.FORGE_E2E_OPENCODE === '1'
const model = process.env.FORGE_E2E_OPENCODE_MODEL ?? 'glm-5.1'
const repoPath = process.env.FORGE_E2E_REPO_PATH ?? resolve('..')

async function expectOk(response: APIResponse, label: string) {
  if (response.ok()) return
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`)
}

async function api<T>(
  request: APIRequestContext,
  method: 'GET' | 'POST',
  path: string,
  data?: unknown,
): Promise<T> {
  const response = await request.fetch(path, { method, data, failOnStatusCode: false })
  await expectOk(response, `${method} ${path}`)
  return (await response.json()) as T
}

async function createOpencodeAgent(request: APIRequestContext, name: string): Promise<Agent> {
  return api<Agent>(request, 'POST', '/api/v1/agents', {
    name,
    executor_type: 'opencode',
    model,
    permission_policy: 'auto',
    max_concurrent_tasks: 1,
  })
}

async function waitForAssistant(
  request: APIRequestContext,
  conversationId: string,
): Promise<ConversationMessage> {
  for (let i = 0; i < 120; i += 1) {
    const page = await api<PaginatedResponse<ConversationMessage>>(
      request,
      'GET',
      `/api/v1/conversations/${conversationId}/messages?limit=20`,
    )
    const assistant = page.items.find((item) => item.role === 'assistant')
    if (assistant && assistant.status !== 'streaming') return assistant
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }
  throw new Error('Timed out waiting for assistant message')
}

async function waitForExecution(
  request: APIRequestContext,
  taskId: string,
  executionId: string,
): Promise<Execution> {
  for (let i = 0; i < 180; i += 1) {
    const page = await api<PaginatedResponse<Execution>>(
      request,
      'GET',
      `/api/v1/tasks/${taskId}/executions`,
    )
    const execution = page.items.find((item) => item.id === executionId)
    if (execution && execution.status !== 'running') return execution
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }
  throw new Error('Timed out waiting for task execution')
}

test.describe('OpenCode GLM live e2e', () => {
  test.skip(!shouldRun, 'Set FORGE_E2E_OPENCODE=1 to run live OpenCode model tests')

  test('chat returns assistant content with glm-5.1', async ({ request }) => {
    const stamp = Date.now()
    const agent = await createOpencodeAgent(request, `opencode-glm-chat-${stamp}`)
    const project = await api<Project>(request, 'POST', '/api/v1/projects', {
      name: `OpenCode GLM Chat ${stamp}`,
    })
    const conversation = await api<Conversation>(
      request,
      'POST',
      `/api/v1/projects/${project.id}/conversations`,
      { agent_id: agent.id, title: 'OpenCode GLM chat', system_prompt: null },
    )

    await api(request, 'POST', `/api/v1/conversations/${conversation.id}/messages`, {
      content: 'Do not edit files. Reply exactly: forge-live-chat-ok',
    })

    const assistant = await waitForAssistant(request, conversation.id)
    expect(assistant.status).toBe('complete')
    expect(assistant.content).toContain('forge-live-chat-ok')
    expect(assistant.error).toBeNull()
  })

  test('manual task execution completes with glm-5.1', async ({ request }) => {
    const stamp = Date.now()
    const agent = await createOpencodeAgent(request, `opencode-glm-task-${stamp}`)
    const project = await api<Project>(request, 'POST', '/api/v1/projects', {
      name: `OpenCode GLM Task ${stamp}`,
    })
    await api<Repo>(request, 'POST', `/api/v1/projects/${project.id}/repos`, {
      name: 'repo',
      remote_url: `file://${repoPath}`,
      local_path: repoPath,
      work_mode: 'direct_merge',
      default_branch: 'main',
    })
    const task = await api<Task>(request, 'POST', `/api/v1/projects/${project.id}/tasks`, {
      title: 'OpenCode GLM task smoke',
      description: 'Do not edit files. Reply exactly: forge-live-task-ok',
    })
    const launch = await api<{ data: { execution: Execution } }>(
      request,
      'POST',
      `/api/v1/tasks/${task.id}/launch`,
      {
        agent_id: agent.id,
        summary: null,
        overrides: { model_id: model, reasoning_effort: null, permission_policy: 'auto' },
      },
    )

    const execution = await waitForExecution(request, task.id, launch.data.execution.id)
    expect(execution.status).toBe('completed')
    expect(execution.summary).toContain('forge-live-task-ok')
    expect(execution.error).toBeNull()
  })
})
