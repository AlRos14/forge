import type { ReactNode } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '@/lib/i18n'
import { useAuthStore } from '@/stores/auth'
import { ProjectCopilotDock } from './ProjectCopilotDock'

type LinkProps = {
  to: string
  params?: Record<string, string>
  className?: string
  title?: string
  children: ReactNode
}

vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, params, className, title, children }: LinkProps) => {
    const href = params
      ? Object.entries(params).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)
      : to
    return (
      <a href={href} className={className} title={title}>
        {children}
      </a>
    )
  },
}))

const task = {
  id: 'task-b',
  project_id: 'project-b',
  title: 'Prepare the Project plan',
}

const earlierTask = {
  id: 'task-a',
  project_id: 'project-b',
  title: 'Earlier task',
}

const userMessage = {
  id: 'user-message-1',
  run_id: 'run-1',
  project_id: 'project-b',
  role: 'user',
  actor_type: 'human',
  actor_id: 'human-1',
  content: 'Please split this into WorkUnits.',
  created_at: '2026-11-04T10:00:00Z',
}

let fetchMock: ReturnType<typeof vi.fn>
let sentBodies: Array<{ path: string; body: Record<string, unknown> }> = []
let requestedPaths: string[] = []
let requestedActivityCursors: Array<string | null> = []
let activityPaginationEnabled = false
let projectMembershipRole = 'member'
let copilotAgentId: string | null = 'agent-1'
let supportedCopilotExecutors = ['claude_code', 'codex', 'opencode']

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}

function runFor(projectId: string, status = 'queued') {
  return {
    id: 'run-1',
    project_id: projectId,
    agent_id: 'agent-1',
    binding_version: 1,
    context_task_id: 'task-b',
    status,
    failure_code: null,
    failure_message: null,
    created_at: '2026-11-04T10:00:00Z',
    started_at: null,
    completed_at: null,
    updated_at: '2026-11-04T10:00:00Z',
  }
}

function projectFor(projectId: string) {
  return {
    id: projectId,
    name: projectId === 'project-b' ? 'Project B' : projectId,
    project_hooks: [],
    primary_repo_id: null,
    owner_id: 'owner-1',
    created_at: '2026-11-01T00:00:00Z',
    updated_at: '2026-11-01T00:00:00Z',
    paused_at: null,
    paused: false,
    version: 1,
  }
}

function projectConfigFor(projectId: string) {
  return {
    project_id: projectId,
    agent_id: copilotAgentId,
    agent_name: copilotAgentId ? `Agent for ${projectId}` : null,
    version: 1,
    active_run: null,
    supported_executor_types: supportedCopilotExecutors,
    supported_daemon_id: 'embedded-host',
  }
}

function installFetchMock() {
  fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), window.location.origin)
    const path = url.pathname.replace('/api/v1', '')
    const method = init?.method ?? 'GET'
    requestedPaths.push(`${method} ${path}`)

    if (path === '/tasks/task-b') return jsonResponse(task)
    if (path === '/tasks/task-a') return jsonResponse(earlierTask)
    if (
      path === '/projects/project-b' ||
      path === '/projects/project-1' ||
      path === '/projects/project-2'
    ) {
      return jsonResponse(projectFor(path.split('/').at(-1)!))
    }
    if (path === '/agents' && method === 'GET') {
      return jsonResponse({
        items: [
          {
            id: 'agent-1',
            owner_id: 'owner-1',
            daemon_id: 'embedded-host',
            executor_type: 'claude_code',
            name: 'Claude Agent',
            model: 'sonnet',
            effective_status: 'active',
          },
          {
            id: 'agent-auto',
            owner_id: 'owner-1',
            daemon_id: null,
            executor_type: 'claude_code',
            name: 'Automatic Claude Agent',
            model: 'sonnet',
            effective_status: 'active',
          },
          {
            id: 'agent-codex',
            owner_id: 'owner-1',
            daemon_id: 'embedded-host',
            executor_type: 'codex',
            name: 'Codex Agent',
            model: null,
            effective_status: 'active',
          },
          {
            id: 'agent-opencode',
            owner_id: 'owner-1',
            daemon_id: 'embedded-host',
            executor_type: 'opencode',
            name: 'OpenCode Agent',
            model: null,
            effective_status: 'active',
          },
          {
            id: 'agent-unsupported',
            owner_id: 'owner-1',
            daemon_id: 'embedded-host',
            executor_type: 'shell',
            name: 'Shell Agent',
            model: null,
            effective_status: 'active',
          },
          {
            id: 'agent-remote',
            owner_id: 'owner-1',
            daemon_id: 'remote-host',
            executor_type: 'codex',
            name: 'Remote Codex Agent',
            model: null,
            effective_status: 'active',
          },
        ],
        next_cursor: null,
        total_count: 3,
      })
    }
    if (path.endsWith('/members')) {
      return jsonResponse([
        {
          id: 'member-1',
          user_id: 'human-1',
          email: 'human@example.test',
          display_name: 'Human',
          role: projectMembershipRole,
          created_at: '2026-11-01T00:00:00Z',
        },
      ])
    }
    if (path.endsWith('/copilot') && method === 'GET') {
      const projectId = path.split('/')[2]!
      return jsonResponse(projectConfigFor(projectId))
    }
    if (path.endsWith('/copilot/messages') && method === 'GET') {
      const projectId = path.split('/')[2]!
      const cursor = url.searchParams.get('cursor')
      if (projectId === 'project-1' && cursor === 'older') {
        return jsonResponse({
          items: [
            {
              id: 'earlier-1',
              run_id: 'old-run',
              project_id: projectId,
              role: 'assistant',
              actor_type: 'agent',
              actor_id: 'agent-1',
              content: 'Earlier turn one',
              created_at: '2026-11-04T08:00:00Z',
            },
            {
              id: 'earlier-2',
              run_id: 'old-run',
              project_id: projectId,
              role: 'assistant',
              actor_type: 'agent',
              actor_id: 'agent-1',
              content: 'Earlier turn two',
              created_at: '2026-11-04T08:30:00Z',
            },
          ],
          next_cursor: null,
        })
      }
      const content = projectId === 'project-1' ? 'First project history' : 'Second project history'
      return jsonResponse({
        items: [
          {
            id: `message-${projectId}`,
            run_id: 'old-run',
            project_id: projectId,
            role: 'assistant',
            actor_type: 'agent',
            actor_id: 'agent-1',
            content,
            created_at: '2026-11-04T09:00:00Z',
          },
          ...(projectId === 'project-1'
            ? [
                {
                  id: 'latest-message-project-1',
                  run_id: 'latest-run',
                  project_id: projectId,
                  role: 'assistant',
                  actor_type: 'agent',
                  actor_id: 'agent-1',
                  content: 'Current latest turn',
                  created_at: '2026-11-04T10:00:00Z',
                },
              ]
            : []),
        ],
        next_cursor: projectId === 'project-1' ? 'older' : null,
      })
    }
    if (path.endsWith('/copilot/activity')) {
      const cursor = url.searchParams.get('cursor')
      requestedActivityCursors.push(cursor)
      if (activityPaginationEnabled) {
        if (cursor === 'older-activity') {
          return jsonResponse({
            items: [
              {
                id: 'activity-created',
                run_id: 'old-run',
                event_type: 'work_unit.created',
                entity_type: 'work_unit',
                entity_id: 'work-unit-1',
                task_id: 'task-b',
                actor_id: 'agent-1',
                requested_by_user_id: 'human-1',
                tool: null,
                outcome: null,
                created_at: '2026-11-04T09:00:00Z',
              },
            ],
            next_cursor: null,
          })
        }
        return jsonResponse({
          items: [
            {
              id: 'activity-dependency-removed',
              run_id: 'latest-run',
              event_type: 'work_unit.dependency_removed',
              entity_type: 'work_unit_dependency',
              entity_id: 'dependency-1',
              task_id: 'task-b',
              actor_id: 'agent-1',
              requested_by_user_id: 'human-1',
              tool: null,
              outcome: null,
              created_at: '2026-11-04T10:00:00Z',
            },
          ],
          next_cursor: 'older-activity',
        })
      }
      return jsonResponse({ items: [], next_cursor: null })
    }
    if (path.endsWith('/copilot/messages') && method === 'POST') {
      const projectId = path.split('/')[2]!
      const body = JSON.parse(String(init?.body)) as Record<string, unknown>
      sentBodies.push({ path, body })
      return jsonResponse({ run: runFor(projectId), user_message: userMessage }, 202)
    }
    if (path.endsWith('/copilot/runs/run-1') && method === 'DELETE') {
      const projectId = path.split('/')[2]!
      return jsonResponse(runFor(projectId, 'cancelled'))
    }
    if (path.endsWith('/copilot/runs/run-1') && method === 'GET') {
      const projectId = path.split('/')[2]!
      return jsonResponse({
        run: runFor(projectId),
        user_message: userMessage,
        assistant_message: null,
      })
    }
    return jsonResponse({ message: `Unhandled test request: ${method} ${path}` }, 404)
  })
  vi.stubGlobal('fetch', fetchMock)
}

function renderDock(props: { projectId?: string; taskId?: string }, onPageAction?: () => void) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  })
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      {onPageAction ? <button onClick={onPageAction}>Task page action</button> : null}
      {children}
    </QueryClientProvider>
  )
  return {
    ...render(<ProjectCopilotDock {...props} />, { wrapper: Wrapper }),
    queryClient,
  }
}

beforeEach(() => {
  void i18n.changeLanguage('en')
  sentBodies = []
  requestedPaths = []
  requestedActivityCursors = []
  activityPaginationEnabled = false
  projectMembershipRole = 'member'
  copilotAgentId = 'agent-1'
  supportedCopilotExecutors = ['claude_code', 'codex', 'opencode']
  installFetchMock()
  useAuthStore.setState({
    user: {
      id: 'human-1',
      email: 'human@example.test',
      display_name: 'Human',
      is_admin: false,
      created_at: '2026-11-01T00:00:00Z',
    },
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
  useAuthStore.setState({ user: null })
})

describe('ProjectCopilotDock', () => {
  it('keeps the desktop page interactive while the pinned Copilot drawer is open', async () => {
    const onPageAction = vi.fn()
    renderDock({ projectId: 'project-1' }, onPageAction)
    fireEvent.click(await screen.findByRole('button', { name: 'Open Project Copilot' }))

    const dialog = await screen.findByRole('dialog', { name: 'Project Copilot panel' })
    expect(dialog.getAttribute('aria-modal')).toBeNull()
    fireEvent.click(
      await screen.findByRole('button', { name: 'Keep Copilot open while you navigate' }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Task page action' }))

    expect(onPageAction).toHaveBeenCalledOnce()
  })

  it('resolves a direct Task route to that Task Project and sends task_id with the message', async () => {
    renderDock({ taskId: 'task-b' })

    await waitFor(() => {
      expect(
        (screen.getByRole('button', { name: 'Open Project Copilot' }) as HTMLButtonElement)
          .disabled,
      ).toBe(false)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Open Project Copilot' }))

    expect(await screen.findByRole('heading', { name: 'Project Copilot' })).toBeTruthy()
    expect(await screen.findByText('Project B · Project conversation')).toBeTruthy()
    expect(await screen.findByText('Prepare the Project plan')).toBeTruthy()
    expect(requestedPaths).toContain('GET /projects/project-b/copilot')
    expect(requestedPaths).not.toContain('GET /projects/project-1/copilot')

    const composer = await screen.findByRole('textbox', { name: 'Message to Project Copilot' })
    fireEvent.change(composer, { target: { value: 'Please split this into WorkUnits.' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))

    await waitFor(() => expect(sentBodies).toHaveLength(1))
    expect(sentBodies[0]).toMatchObject({
      path: '/projects/project-b/copilot/messages',
      body: {
        content: 'Please split this into WorkUnits.',
        task_id: 'task-b',
      },
    })
    await waitFor(() => {
      expect(
        (screen.getByRole('button', { name: 'Stop this turn' }) as HTMLButtonElement).disabled,
      ).toBe(false)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Stop this turn' }))
    await waitFor(() =>
      expect(requestedPaths).toContain('DELETE /projects/project-b/copilot/runs/run-1'),
    )
  })

  it('keeps transcript and draft state isolated when the Project changes', async () => {
    const view = renderDock({ projectId: 'project-1' })
    fireEvent.click(await screen.findByRole('button', { name: 'Open Project Copilot' }))
    expect(await screen.findByText('First project history')).toBeTruthy()
    fireEvent.change(await screen.findByRole('textbox', { name: 'Message to Project Copilot' }), {
      target: { value: 'Draft for the first Project' },
    })

    view.rerender(<ProjectCopilotDock projectId="project-2" />)
    fireEvent.click(await screen.findByRole('button', { name: 'Open Project Copilot' }))

    expect(await screen.findByText('Second project history')).toBeTruthy()
    expect(screen.queryByText('First project history')).toBeNull()
    expect(
      (
        (await screen.findByRole('textbox', {
          name: 'Message to Project Copilot',
        })) as HTMLTextAreaElement
      ).value,
    ).toBe('')
  })

  it('shows the newest page first and prepends older messages without moving the viewport', async () => {
    renderDock({ projectId: 'project-1' })
    fireEvent.click(await screen.findByRole('button', { name: 'Open Project Copilot' }))

    const log = await screen.findByRole('log', { name: 'Project Copilot conversation' })
    await screen.findByText('Current latest turn')
    const articles = () => Array.from(log.querySelectorAll('article'))
    expect(articles().at(-1)?.textContent).toContain('Current latest turn')

    const scroller = log.parentElement!
    Object.defineProperty(scroller, 'scrollHeight', {
      configurable: true,
      get: () => 200 + articles().length * 100,
    })
    Object.defineProperty(scroller, 'clientHeight', { configurable: true, value: 150 })
    scroller.scrollTop = 25

    fireEvent.click(await screen.findByRole('button', { name: 'Load older messages' }))
    await screen.findByText('Earlier turn one')
    await waitFor(() => expect(scroller.scrollTop).toBe(225))
    expect(articles().map((article) => article.textContent)).toEqual([
      expect.stringContaining('Earlier turn one'),
      expect.stringContaining('Earlier turn two'),
      expect.stringContaining('First project history'),
      expect.stringContaining('Current latest turn'),
    ])
  })

  it('keeps a pinned draft attached to its original Task after navigating to another Task', async () => {
    const view = renderDock({ taskId: 'task-a' })
    await waitFor(() => {
      expect(
        (screen.getByRole('button', { name: 'Open Project Copilot' }) as HTMLButtonElement)
          .disabled,
      ).toBe(false)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Open Project Copilot' }))
    fireEvent.click(
      await screen.findByRole('button', { name: 'Keep Copilot open while you navigate' }),
    )
    fireEvent.change(await screen.findByRole('textbox', { name: 'Message to Project Copilot' }), {
      target: { value: 'Continue work on the earlier task.' },
    })

    view.rerender(<ProjectCopilotDock taskId="task-b" />)

    expect(await screen.findByText('This draft is still linked to Earlier task.')).toBeTruthy()
    expect(await screen.findByText('Prepare the Project plan')).toBeTruthy()
    expect(await screen.findByText('Draft context: Earlier task')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    await waitFor(() => expect(sentBodies).toHaveLength(1))
    expect(sentBodies[0]?.body).toMatchObject({ task_id: 'task-a' })
  })

  it('organizes a Task with a scoped request that stops before worker execution', async () => {
    renderDock({ taskId: 'task-b' })
    await waitFor(() => {
      expect(
        (screen.getByRole('button', { name: 'Open Project Copilot' }) as HTMLButtonElement)
          .disabled,
      ).toBe(false)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Open Project Copilot' }))
    await waitFor(() => {
      expect(
        (screen.getByRole('button', { name: 'Organize this task' }) as HTMLButtonElement).disabled,
      ).toBe(false)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Organize this task' }))

    await waitFor(() => expect(sentBodies).toHaveLength(1))
    expect(sentBodies[0]?.body).toMatchObject({ task_id: 'task-b' })
    expect(sentBodies[0]?.body.content).toContain('existing TaskRoles')
    expect(sentBodies[0]?.body.content).toContain('Do not start or execute workers')
  })

  it('renders the Copilot in Spanish when Spanish is selected', async () => {
    await i18n.changeLanguage('es')
    renderDock({ projectId: 'project-1' })
    fireEvent.click(await screen.findByRole('button', { name: 'Abrir el copiloto del proyecto' }))

    expect(await screen.findByRole('heading', { name: 'Copiloto del proyecto' })).toBeTruthy()
    expect(await screen.findByText('project-1 · Conversación del proyecto')).toBeTruthy()
  })

  it('loads older activity and labels dependency removal events', async () => {
    activityPaginationEnabled = true
    renderDock({ projectId: 'project-1' })
    fireEvent.click(await screen.findByRole('button', { name: 'Open Project Copilot' }))
    fireEvent.click(await screen.findByText('Project activity'))

    expect(await screen.findByText('Removed a WorkUnit dependency')).toBeTruthy()
    fireEvent.click(await screen.findByRole('button', { name: 'Load older activity' }))

    expect(await screen.findByText('Created a WorkUnit')).toBeTruthy()
    expect(requestedActivityCursors).toContain('older-activity')
  })

  it('offers every server-supported harness on the authorized local host', async () => {
    projectMembershipRole = 'admin'
    renderDock({ projectId: 'project-1' })
    fireEvent.click(await screen.findByRole('button', { name: 'Open Project Copilot' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Change' }))

    expect(
      await screen.findByText(
        "Choose an Agent owned by this Project's owner whose harness can be restricted to Copilot tools. Agents without a host pin use the supported local host automatically.",
      ),
    ).toBeTruthy()
    const agentSelector = screen.getByRole('button', { name: 'Agent' })
    await waitFor(() => expect((agentSelector as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(agentSelector)

    expect(await screen.findByRole('option', { name: /^Claude Agent/ })).toBeTruthy()
    expect(await screen.findByRole('option', { name: /^Automatic Claude Agent/ })).toBeTruthy()
    expect(await screen.findByRole('option', { name: /Codex Agent/ })).toBeTruthy()
    expect(await screen.findByRole('option', { name: /OpenCode Agent/ })).toBeTruthy()
    expect(screen.queryByRole('option', { name: /Shell Agent/ })).toBeNull()
    expect(screen.queryByRole('option', { name: /Remote Codex Agent/ })).toBeNull()
  })

  it('preselects the only eligible Agent when there is no current assignment', async () => {
    projectMembershipRole = 'admin'
    copilotAgentId = null
    supportedCopilotExecutors = ['opencode']
    renderDock({ projectId: 'project-1' })
    fireEvent.click(await screen.findByRole('button', { name: 'Open Project Copilot' }))

    const save = await screen.findByRole('button', { name: 'Save' })
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false))
    expect(screen.getByRole('button', { name: 'Agent' }).textContent).toContain(
      'OpenCode Agent',
    )
  })

  it('uses a modal drawer on mobile and keeps the exact Task attached', async () => {
    const previousWidth = window.innerWidth
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 390 })
    try {
      renderDock({ taskId: 'task-b' })
      await waitFor(() => {
        expect(
          (screen.getByRole('button', { name: 'Open Project Copilot' }) as HTMLButtonElement)
            .disabled,
        ).toBe(false)
      })
      fireEvent.click(screen.getByRole('button', { name: 'Open Project Copilot' }))

      const dialog = await screen.findByRole('dialog', { name: 'Project Copilot panel' })
      expect(dialog.getAttribute('aria-modal')).toBe('true')
      expect(await screen.findByText('Prepare the Project plan')).toBeTruthy()
    } finally {
      Object.defineProperty(window, 'innerWidth', { configurable: true, value: previousWidth })
    }
  })
})
