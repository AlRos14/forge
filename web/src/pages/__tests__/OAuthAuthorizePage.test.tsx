import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { OAuthAuthorizePage } from '@/pages/OAuthAuthorizePage'
import { useAuthStore } from '@/stores/auth'

const router = vi.hoisted(() => ({
  navigate: vi.fn(),
  search: {} as Partial<Record<string, string>>,
}))

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => router.navigate,
  useSearch: () => router.search,
}))

const oauthParams = {
  response_type: 'code',
  client_id: 'client-1',
  redirect_uri: 'http://127.0.0.1:4321/callback',
  resource: 'http://localhost:8080/mcp',
  scope: 'mcp',
  state: 'state-1',
  code_challenge: 'challenge-1',
  code_challenge_method: 'S256',
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  })

  render(
    <QueryClientProvider client={queryClient}>
      <OAuthAuthorizePage />
    </QueryClientProvider>,
  )
}

describe('OAuthAuthorizePage', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    router.navigate.mockReset()
    router.search = { ...oauthParams }
    useAuthStore.setState({ accessToken: null, refreshToken: null, user: null })
    localStorage.clear()
    window.history.replaceState(null, '', '/')
  })

  it('redirects unauthenticated OAuth requests to login while preserving params', async () => {
    const fetchMock = vi.spyOn(window, 'fetch')

    renderPage()

    await waitFor(() =>
      expect(router.navigate).toHaveBeenCalledWith({
        to: '/login',
        search: {
          redirect: '/oauth/authorize/consent',
          redirect_params: JSON.stringify(oauthParams),
        },
        replace: true,
      }),
    )
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('fetches context and submits approve or cancel decisions', async () => {
    useAuthStore.setState({ accessToken: 'access-token', refreshToken: 'refresh-token' })
    const approveBodies: Array<Record<string, unknown>> = []
    const fetchMock = vi.spyOn(window, 'fetch').mockImplementation(async (input, init) => {
      const url = input instanceof URL ? input : new URL(String(input), window.location.origin)

      if (url.pathname === '/api/v1/oauth/authorize/context') {
        return new Response(
          JSON.stringify({
            client_id: 'client-1',
            client_name: 'Claude Code',
            redirect_uri: 'http://127.0.0.1:4321/callback',
            resource: 'http://localhost:8080/mcp',
            scopes: ['mcp'],
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        )
      }

      if (url.pathname === '/api/v1/oauth/authorize/approve') {
        const body = JSON.parse(String(init?.body)) as Record<string, unknown>
        approveBodies.push(body)
        return new Response(
          JSON.stringify({
            redirect_to: body.decision === 'approve' ? '#approved' : '#denied',
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        )
      }

      return new Response('Not found', { status: 404 })
    })

    renderPage()

    await screen.findByText('Claude Code')
    const contextCall = fetchMock.mock.calls.find(([input]) => {
      const url = input instanceof URL ? input : new URL(String(input), window.location.origin)
      return url.pathname === '/api/v1/oauth/authorize/context'
    })
    expect(contextCall).toBeTruthy()
    const contextUrl =
      contextCall?.[0] instanceof URL
        ? contextCall[0]
        : new URL(String(contextCall?.[0]), window.location.origin)
    expect(contextUrl.searchParams.get('client_id')).toBe('client-1')

    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))

    await waitFor(() => expect(approveBodies).toHaveLength(1))
    expect(approveBodies[0]).toMatchObject({ ...oauthParams, decision: 'approve' })
    await waitFor(() => expect(window.location.hash).toBe('#approved'))

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    await waitFor(() => expect(approveBodies).toHaveLength(2))
    expect(approveBodies[1]).toMatchObject({ ...oauthParams, decision: 'deny' })
    await waitFor(() => expect(window.location.hash).toBe('#denied'))
  })
})
