import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useAuthStore } from '@/stores/auth'
import type {
  TerminalAttachTokenResponse,
  TerminalAvailability,
  TerminalSessionResponse,
  UserResponse,
} from '@/types/generated'
import { TaskTerminalPanel } from './task-terminal-panel'

const terminalApi = vi.hoisted(() => ({
  createTerminalSession: vi.fn(),
  getTerminalAvailability: vi.fn(),
  issueTerminalAttachToken: vi.fn(),
  listTerminalSessions: vi.fn(),
  resizeTerminalSession: vi.fn(),
  terminalWebSocketUrl: vi.fn((sessionId: string, attachToken: string) => {
    const url = new URL(`/api/v1/terminals/${sessionId}/ws`, window.location.origin)
    url.protocol = 'ws:'
    url.searchParams.set('attach_token', attachToken)
    return url.toString()
  }),
  terminateTerminalSession: vi.fn(),
}))

vi.mock('@/api/terminals', () => terminalApi)

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {}
  },
}))

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 24

    dispose() {}
    loadAddon() {}
    open() {}
    write() {}
    onData() {
      return { dispose() {} }
    }
  },
}))

vi.mock('sonner', () => ({
  toast: {
    error: vi.fn(),
  },
}))

const now = '2026-05-20T12:00:00Z'
const currentUser: UserResponse = {
  id: 'user-1',
  email: 'user@example.com',
  display_name: null,
  is_admin: false,
  created_at: now,
}

const availability: TerminalAvailability = {
  enabled: true,
  workspace_ready: true,
  daemon_reachable: true,
  active_execution: false,
  session_count_for_task: 1,
  session_count_for_user: 1,
  max_sessions_per_task: 2,
  max_sessions_per_user: 4,
  can_create: true,
  reason: null,
}

const runningSession: TerminalSessionResponse = {
  id: 'term-1',
  task_id: 'task-1',
  workspace_id: 'workspace-1',
  daemon_id: null,
  status: 'running',
  rows: 24,
  cols: 80,
  exit_code: null,
  exit_signal: null,
  exit_reason: null,
  created_at: now,
  started_at: now,
  last_activity_at: now,
  ended_at: null,
  created_by_user_id: currentUser.id,
}

const attachToken: TerminalAttachTokenResponse = {
  attach_token: 'attach-token',
  expires_at: '2026-05-20T12:01:00Z',
  ws_url: '/api/v1/terminals/term-1/ws?attach_token=attach-token',
  session_id: runningSession.id,
}

describe('TaskTerminalPanel', () => {
  const websocketUrls: string[] = []

  beforeEach(() => {
    terminalApi.createTerminalSession.mockReset()
    terminalApi.getTerminalAvailability.mockResolvedValue(availability)
    terminalApi.issueTerminalAttachToken.mockReset()
    terminalApi.listTerminalSessions.mockResolvedValue([runningSession])
    terminalApi.resizeTerminalSession.mockResolvedValue(runningSession)
    terminalApi.terminalWebSocketUrl.mockClear()
    terminalApi.terminateTerminalSession.mockReset()
    websocketUrls.length = 0

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
        websocketUrls.push(this.url)
        window.setTimeout(() => {
          this.readyState = MockWebSocket.OPEN
          const event = new Event('open')
          this.onopen?.(event)
          this.dispatchEvent(event)
        }, 0)
      }

      close() {
        this.readyState = MockWebSocket.CLOSED
        const event = new Event('close') as CloseEvent
        this.onclose?.(event)
        this.dispatchEvent(event)
      }

      send() {}
    }

    class MockResizeObserver {
      disconnect() {}
      observe() {}
      unobserve() {}
    }

    vi.stubGlobal('WebSocket', MockWebSocket)
    vi.stubGlobal('ResizeObserver', MockResizeObserver)
    useAuthStore.setState({
      accessToken: 'access-token',
      refreshToken: 'refresh-token',
      user: currentUser,
    })
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.clearAllMocks()
    useAuthStore.getState().clearAuth()
  })

  it('keeps repeated reattach clicks to one token request and socket', async () => {
    let resolveAttachToken: (value: TerminalAttachTokenResponse) => void = () => {}
    terminalApi.issueTerminalAttachToken.mockReturnValue(
      new Promise<TerminalAttachTokenResponse>((resolve) => {
        resolveAttachToken = resolve
      }),
    )

    render(<TaskTerminalPanel taskId="task-1" />)

    const reattach = await screen.findByRole('button', {
      name: 'Reattach to running session',
    })
    fireEvent.click(reattach)
    fireEvent.click(reattach)

    expect(terminalApi.issueTerminalAttachToken).toHaveBeenCalledTimes(1)

    resolveAttachToken(attachToken)

    await waitFor(() => {
      expect(websocketUrls).toHaveLength(1)
    })
  })
})
