import { expect, test, type APIRequestContext, type APIResponse } from './fixtures'

type PaginatedResponse<T> = {
  items: T[]
  has_more: boolean
  next_cursor?: string | null
}

type Project = { id: string; name: string }
type Agent = { id: string; name: string; executor_type: string }
type Conversation = { id: string; version: number; title: string; agent_id: string | null }
type ConversationMessage = {
  id: string
  role: 'user' | 'assistant' | 'system'
  status: 'complete' | 'streaming' | 'failed' | 'cancelled'
  content: string
}

type SendMessageResponse = {
  user_message: ConversationMessage
  assistant_message: ConversationMessage
}

async function expectOk(response: APIResponse, label: string) {
  if (response.ok()) return
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`)
}

async function api<T>(
  request: APIRequestContext,
  method: 'GET' | 'POST' | 'PATCH' | 'DELETE',
  path: string,
  data?: unknown,
): Promise<T> {
  const response = await request.fetch(path, { method, data, failOnStatusCode: false })
  await expectOk(response, `${method} ${path}`)
  if (response.status() === 204) return undefined as T
  return (await response.json()) as T
}

async function createProject(request: APIRequestContext, name: string): Promise<Project> {
  return api<Project>(request, 'POST', '/api/v1/projects', { name })
}

async function createConversationCapableAgent(
  request: APIRequestContext,
  name: string,
): Promise<Agent> {
  const candidateTypes = ['codex', 'claude_code', 'opencode']
  for (const executorType of candidateTypes) {
    const response = await request.post('/api/v1/agents', {
      data: { name: `${name}-${executorType}`, executor_type: executorType },
      failOnStatusCode: false,
    })
    if (response.ok()) return (await response.json()) as Agent
  }
  return api<Agent>(request, 'POST', '/api/v1/agents', {
    name: `${name}-shell`,
    executor_type: 'shell',
  })
}

async function waitForMessages(
  request: APIRequestContext,
  conversationId: string,
  predicate: (messages: ConversationMessage[]) => boolean,
): Promise<ConversationMessage[]> {
  for (let i = 0; i < 40; i += 1) {
    const page = await api<PaginatedResponse<ConversationMessage>>(
      request,
      'GET',
      `/api/v1/conversations/${conversationId}/messages?limit=200`,
    )
    if (predicate(page.items)) return page.items
    await new Promise((resolve) => setTimeout(resolve, 250))
  }

  const page = await api<PaginatedResponse<ConversationMessage>>(
    request,
    'GET',
    `/api/v1/conversations/${conversationId}/messages?limit=200`,
  )
  return page.items
}

test.describe('project chat e2e', () => {
  test('creates conversation, sends message, and persists thread', async ({ request }) => {
    const project = await createProject(request, `Chat E2E ${Date.now()}`)
    const agent = await createConversationCapableAgent(request, 'chat-e2e-agent')

    const conversation = await api<Conversation>(
      request,
      'POST',
      `/api/v1/projects/${project.id}/conversations`,
      { agent_id: agent.id, title: 'E2E chat thread' },
    )

    const send = await api<SendMessageResponse>(
      request,
      'POST',
      `/api/v1/conversations/${conversation.id}/messages`,
      { content: 'Can you summarize priorities?' },
    )
    expect(send.user_message.role).toBe('user')
    expect(send.assistant_message.role).toBe('assistant')

    const messages = await waitForMessages(request, conversation.id, (items) => {
      const hasUser = items.some((item) => item.role === 'user')
      const hasAssistantTerminal = items.some(
        (item) => item.role === 'assistant' && item.status !== 'streaming',
      )
      return hasUser && hasAssistantTerminal
    })

    expect(messages.some((item) => item.role === 'user')).toBeTruthy()
    expect(messages.some((item) => item.role === 'assistant')).toBeTruthy()
  })

  test('changes agent mid-conversation and records system message', async ({ request }) => {
    const project = await createProject(request, `Chat Agent Switch ${Date.now()}`)
    const agentA = await createConversationCapableAgent(request, 'chat-switch-a')
    const agentB = await createConversationCapableAgent(request, 'chat-switch-b')

    const conversation = await api<Conversation>(
      request,
      'POST',
      `/api/v1/projects/${project.id}/conversations`,
      { agent_id: agentA.id, title: 'Switch test' },
    )

    const updated = await api<Conversation>(
      request,
      'PATCH',
      `/api/v1/conversations/${conversation.id}`,
      { version: conversation.version, agent_id: agentB.id },
    )

    expect(updated.agent_id).toBe(agentB.id)

    const messages = await waitForMessages(request, conversation.id, (items) =>
      items.some((item) => item.role === 'system' && item.content.includes('agent_changed')),
    )

    expect(
      messages.some((item) => item.role === 'system' && item.content.includes('agent_changed')),
    ).toBeTruthy()
  })

  test('cancels streaming response and preserves partial content when cancellation is available', async ({
    request,
  }) => {
    const project = await createProject(request, `Chat Cancel ${Date.now()}`)
    const agent = await createConversationCapableAgent(request, 'chat-cancel-agent')

    const conversation = await api<Conversation>(
      request,
      'POST',
      `/api/v1/projects/${project.id}/conversations`,
      { agent_id: agent.id, title: 'Cancel test' },
    )

    await api<SendMessageResponse>(
      request,
      'POST',
      `/api/v1/conversations/${conversation.id}/messages`,
      { content: 'Please start a long response' },
    )

    const partialMessages = await waitForMessages(request, conversation.id, (items) =>
      items.some(
        (item) =>
          item.role === 'assistant' && item.status === 'streaming' && item.content.length > 0,
      ),
    )
    const hasPartialContent = partialMessages.some(
      (item) => item.role === 'assistant' && item.status === 'streaming' && item.content.length > 0,
    )
    if (!hasPartialContent) {
      test.skip(true, 'No partial streaming response in this environment')
      return
    }

    const cancel = await request.post(`/api/v1/conversations/${conversation.id}/cancel`, {
      failOnStatusCode: false,
    })

    if (cancel.status() === 409) {
      test.skip(true, 'No active streaming response in this environment')
      return
    }

    expect(cancel.status()).toBe(204)

    const messages = await waitForMessages(request, conversation.id, (items) =>
      items.some((item) => item.role === 'assistant' && item.status === 'cancelled'),
    )

    const cancelled = messages.find(
      (item) => item.role === 'assistant' && item.status === 'cancelled',
    )
    expect(cancelled).toBeTruthy()
    expect((cancelled?.content ?? '').length).toBeGreaterThan(0)
  })
})
