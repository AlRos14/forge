import { fireEvent, render, screen, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { FederatedAgentsPage } from '@/pages/FederatedAgentsPage'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import type { FederatedAgent } from '@/features/federation/types'
import type {
  ProviderAuthorizationOperationResponse,
  ProviderEntryResponse,
} from '@/types/generated'

const createEmbeddedAgent = vi.fn().mockResolvedValue({})
const registerHarnessAgent = vi.fn().mockResolvedValue({})
const connectProfile = vi.fn().mockResolvedValue({})
const createProviderEntry = vi.fn()
const removeProviderEntry = vi.fn().mockResolvedValue({ provider_revocation: 'succeeded' })
const cancelAuthorization = vi.fn().mockResolvedValue({})
const testProviderEntry = vi.fn().mockResolvedValue({
  status: 'ok',
  latency_ms: 123,
  message: null,
  checked_at: '2026-08-15T00:00:00Z',
})

vi.mock('@/features/federation/api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/features/federation/api')>()),
  testProviderEntry: (id: string) => testProviderEntry(id) as Promise<unknown>,
}))
let authorizationOperation: ProviderAuthorizationOperationResponse | undefined
const startAuthorization = vi.fn().mockImplementation(async () => {
  authorizationOperation = {
    id: 'authorization-1',
    provider: 'openai',
    method: 'device_oauth',
    state: 'polling',
    authorization_url: 'https://auth.example/device',
    user_code: 'ABCD-EFGH',
    expires_at: '2026-08-14T16:00:00Z',
    poll_interval_seconds: 5,
    credential_handle_id: null,
    error_code: null,
    error_message: null,
    version: 1,
    created_at: '2026-08-14T15:50:00Z',
    updated_at: '2026-08-14T15:50:00Z',
    completed_at: null,
  }
  return authorizationOperation
})

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
  useSearch: () => ({}),
}))
vi.mock('@/stores/auth', () => ({
  useAuthStore: (selector: (state: { user: null }) => unknown) => selector({ user: null }),
}))
vi.mock('@/features/federation/hooks', () => ({
  isVersionConflict: () => false,
  useFederatedAgentsQuery: () => ({
    data: { items: [agent], has_more: false },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useAgentProfilesQuery: () => ({ data: [], isLoading: false, isError: false }),
  useAgentSessionsQuery: () => ({ data: [], isLoading: false, isError: false }),
  useSelectAgentProfileMutation: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useCreateEmbeddedAgentMutation: () => ({ mutateAsync: createEmbeddedAgent, isPending: false }),
  useRegisterHarnessAgentMutation: () => ({
    mutateAsync: registerHarnessAgent,
    isPending: false,
  }),
  useConnectEmbeddedProfileMutation: () => ({
    mutateAsync: connectProfile,
    isPending: false,
  }),
  useAgentProviderCapabilitiesQuery: () => ({
    data: {
      items: [
        {
          provider: 'openai',
          display_name: 'OpenAI',
          default_base_url: 'https://api.openai.com/v1',
          default_model: 'gpt-5',
          model_discovery: true,
          credential_methods: [
            {
              method: 'api_key',
              action_label: 'Use OpenAI API key',
              support_level: 'stable',
              configured: true,
              setup_guidance: null,
              boundary_note: 'OpenAI Platform credentials.',
              runtimes: [
                { runtime: 'direct', support_level: 'stable', reason: null },
                { runtime: 'codex', support_level: 'stable', reason: null },
              ],
            },
            {
              method: 'browser_oauth',
              action_label: 'Continue with ChatGPT',
              support_level: 'experimental',
              configured: true,
              setup_guidance: null,
              boundary_note: 'Direct ChatGPT adapter.',
              runtimes: [
                { runtime: 'direct', support_level: 'experimental', reason: null },
                {
                  runtime: 'codex',
                  support_level: 'unavailable',
                  reason:
                    "OAuth handoff into the Codex CLI is not supported; use the CLI's own login",
                },
              ],
            },
            {
              method: 'device_oauth',
              action_label: 'Use ChatGPT device code',
              support_level: 'experimental',
              configured: true,
              setup_guidance: null,
              boundary_note: 'Device-code fallback.',
              runtimes: [{ runtime: 'direct', support_level: 'experimental', reason: null }],
            },
          ],
        },
      ],
    },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useProvidersQuery: () => ({
    data: { items: providerEntries, cli_runtimes: cliRuntimes },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useCreateProviderEntryMutation: () => ({ mutateAsync: createProviderEntry, isPending: false }),
  useRenameProviderEntryMutation: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useRemoveProviderEntryMutation: () => ({ mutateAsync: removeProviderEntry, isPending: false }),
  useProviderAuthorizationQuery: () => ({ data: authorizationOperation }),
  useStartProviderAuthorizationMutation: () => ({
    mutateAsync: startAuthorization,
    isPending: false,
  }),
  useCancelProviderAuthorizationMutation: () => ({
    mutateAsync: cancelAuthorization,
    isPending: false,
  }),
  useMainAgentBindingQuery: () => ({
    data: undefined,
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useSetMainAgentBindingMutation: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useContextManifestDiscoveryQuery: () => ({
    data: [],
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: vi.fn(),
  }),
  useContextManifestQuery: () => ({
    data: undefined,
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: vi.fn(),
  }),
}))
vi.mock('@/features/agent-chat/hooks', () => ({
  useAgentChatsQuery: () => ({
    data: { items: chatEntries },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
}))

const chatEntries: AgentChatEntry[] = [
  {
    chat_id: 'main-chat',
    kind: 'main',
    project_id: null,
    project_name: null,
    identity_id: 'agent-1',
    identity_name: 'Forge Guide',
    binding_state: 'active',
    chat_status: 'ready',
    unread_count: 0n,
    pending_turn_count: 0n,
    last_message_at: null,
  },
]

const providerEntries: ProviderEntryResponse[] = [
  {
    id: 'credential-1',
    provider: 'openai',
    label: 'Personal ChatGPT',
    credential_method: 'oauth_bundle',
    status: 'configured',
    base_url: 'https://chatgpt.com/backend-api/codex',
    provider_account_id: 'acct-123',
    used_by: [{ agent_id: 'agent-1', agent_name: 'Forge Guide', runtime: 'direct' }],
    last_used_at: '2026-08-14T12:00:00Z',
    version: 1,
    created_at: '2026-08-12T11:00:00Z',
    updated_at: '2026-08-12T12:00:00Z',
  },
  {
    id: 'credential-2',
    provider: 'openai',
    label: 'Work key',
    credential_method: 'api_key',
    status: 'configured',
    base_url: 'https://api.openai.com/v1',
    provider_account_id: null,
    used_by: [],
    last_used_at: null,
    version: 1,
    created_at: '2026-08-12T11:00:00Z',
    updated_at: '2026-08-12T12:00:00Z',
  },
]

const cliRuntimes = [
  {
    kind: 'claude_code',
    daemon_id: 'daemon-1',
    daemon_hostname: 'forge-host',
    daemon_status: 'online',
    availability: 'unauthenticated',
    version: '2.1.0',
    login_hint: 'Run `claude` on the host and complete its login',
    used_by: [],
  },
]

const agent: FederatedAgent = {
  id: 'agent-1',
  name: 'Forge Guide',
  description: 'A bounded account assistant.',
  profile_id: 'profile-1',
  backend_kind: 'native',
  executor_type: 'embedded',
  provider: 'openai',
  model: 'gpt-test',
  reasoning_effort: null,
  permission_policy: 'scoped_proposals',
  prompt_template: null,
  capabilities: ['read_account'],
  config_json: {},
  credential_handle_id: 'credential-1',
  daemon_id: null,
  max_concurrent_tasks: 1,
  status: 'idle',
  active_task_count: 0,
  effective_status: 'ready',
  total_runs: 4,
  avg_duration_ms: 500,
  success_rate: 1,
  is_default: false,
  paused: false,
  owner_id: 'user-1',
  visibility: 'private',
  version: 1,
  created_at: '2026-08-12T11:00:00Z',
  updated_at: '2026-08-12T12:00:00Z',
}

function openProvidersTab() {
  fireEvent.click(screen.getByRole('tab', { name: /providers/i }))
}

describe('FederatedAgentsPage', () => {
  beforeEach(() => {
    authorizationOperation = undefined
    createEmbeddedAgent.mockClear()
    registerHarnessAgent.mockClear()
    connectProfile.mockClear()
    createProviderEntry.mockClear()
    createProviderEntry.mockResolvedValue(providerEntries[1])
    removeProviderEntry.mockClear()
    startAuthorization.mockClear()
    cancelAuthorization.mockClear()
  })

  it('defaults to the agents roster and keeps bindings on their own tab', () => {
    render(<FederatedAgentsPage />)
    expect(screen.getByRole('tab', { name: /agents/i }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByText('Forge Guide')).toBeTruthy()
    expect(screen.queryByText('Main and Project Agent bindings')).toBeNull()
    fireEvent.click(screen.getByRole('tab', { name: /bindings/i }))
    expect(
      screen.getByRole('tab', { name: /bindings/i }).getAttribute('aria-selected'),
    ).toBe('true')
    expect(screen.getByText('Main and Project Agent bindings')).toBeTruthy()
    expect(screen.getByText('Global · Main')).toBeTruthy()
  })

  it('lists provider entries and CLI runtimes with usage on the providers tab', () => {
    render(<FederatedAgentsPage />)
    openProvidersTab()
    expect(screen.getByText('Personal ChatGPT')).toBeTruthy()
    expect(screen.getByText('Work key')).toBeTruthy()
    expect(screen.getByRole('button', { name: /used by 1 agent/i })).toBeTruthy()
    expect(screen.getByText(/Claude Code harness/)).toBeTruthy()
    expect(screen.getByText(/Run `claude` on the host/)).toBeTruthy()
  })

  it('creates an API-key provider entry step by step and tests the connection', async () => {
    render(<FederatedAgentsPage />)
    openProvidersTab()
    fireEvent.click(screen.getByRole('button', { name: /add provider/i }))
    const wizard = within(screen.getByRole('dialog'))
    expect(wizard.getByText(/step 1 of 4/i)).toBeTruthy()
    fireEvent.click(wizard.getByRole('button', { name: /OpenAI/i }))
    expect(wizard.getByText(/step 2 of 4/i)).toBeTruthy()
    fireEvent.click(wizard.getByRole('button', { name: /use openai api key/i }))
    expect(wizard.getByText(/step 3 of 4/i)).toBeTruthy()
    fireEvent.change(wizard.getByLabelText('API key'), { target: { value: 'secret-value' } })
    fireEvent.click(wizard.getByRole('button', { name: /^add provider$/i }))
    await vi.waitFor(() =>
      expect(createProviderEntry).toHaveBeenCalledWith(
        expect.objectContaining({ provider: 'openai', credential: 'secret-value' }),
      ),
    )
    expect(createEmbeddedAgent).not.toHaveBeenCalled()
    expect(registerHarnessAgent).not.toHaveBeenCalled()
    expect(await screen.findByText(/is connected\. No agent was created\./)).toBeTruthy()
    await vi.waitFor(() => expect(testProviderEntry).toHaveBeenCalledWith('credential-2'))
    expect(await screen.findByText(/Provider responding · 123 ms/)).toBeTruthy()
    expect(
      screen.getByRole('button', { name: /create an agent with this provider/i }),
    ).toBeTruthy()
  })

  it('renders and cancels a device authorization operation without exposing tokens', async () => {
    render(<FederatedAgentsPage />)
    openProvidersTab()
    fireEvent.click(screen.getByRole('button', { name: /add provider/i }))
    const wizard = within(screen.getByRole('dialog'))
    fireEvent.click(wizard.getByRole('button', { name: /OpenAI/i }))
    fireEvent.click(wizard.getByRole('button', { name: /use chatgpt device code/i }))
    fireEvent.click(wizard.getByRole('button', { name: /start authorization/i }))

    expect(await screen.findByText('ABCD-EFGH')).toBeTruthy()
    expect(screen.queryByText(/access_token|refresh_token/i)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /cancel authorization/i }))
    await vi.waitFor(() =>
      expect(cancelAuthorization).toHaveBeenCalledWith({
        id: 'authorization-1',
        input: { expected_version: 1 },
      }),
    )
  })

  it('starts only one provider authorization for a repeated form submission', () => {
    startAuthorization.mockReturnValueOnce(new Promise(() => {}))
    render(<FederatedAgentsPage />)
    openProvidersTab()
    fireEvent.click(screen.getByRole('button', { name: /add provider/i }))
    const wizard = within(screen.getByRole('dialog'))
    fireEvent.click(wizard.getByRole('button', { name: /OpenAI/i }))
    fireEvent.click(wizard.getByRole('button', { name: /use chatgpt device code/i }))
    const form = screen.getByRole('button', { name: /start authorization/i }).closest('form')
    expect(form).toBeTruthy()
    fireEvent.submit(form!)
    fireEvent.submit(form!)

    expect(startAuthorization).toHaveBeenCalledTimes(1)
  })

  it('warns with dependent agents before disconnecting an entry', async () => {
    render(<FederatedAgentsPage />)
    openProvidersTab()
    const disconnectButtons = screen.getAllByRole('button', { name: /^disconnect$/i })
    fireEvent.click(disconnectButtons[0])
    expect(screen.getByText(/1 agent reference this entry \(Forge Guide\)/)).toBeTruthy()
    const confirm = screen
      .getByRole('alertdialog')
      .querySelector('button') as HTMLButtonElement
    fireEvent.click(confirm)
    await vi.waitFor(() =>
      expect(removeProviderEntry).toHaveBeenCalledWith({ handleId: 'credential-1', version: 1 }),
    )
  })

  it('creates a direct agent from a provider entry through the wizard', async () => {
    render(<FederatedAgentsPage />)
    fireEvent.click(screen.getByRole('button', { name: /new agent/i }))
    const wizard = within(screen.getByRole('dialog'))
    fireEvent.click(wizard.getByRole('button', { name: /Openai · Work key/i }))
    expect(wizard.getByText('Codex CLI harness')).toBeTruthy()
    fireEvent.click(wizard.getByRole('button', { name: /Direct · built-in runtime/i }))
    fireEvent.change(wizard.getByLabelText('Agent name'), { target: { value: 'Main guide' } })
    fireEvent.change(wizard.getByLabelText('Model'), { target: { value: 'gpt-5' } })
    fireEvent.click(wizard.getByRole('button', { name: /create agent/i }))
    await vi.waitFor(() =>
      expect(createEmbeddedAgent).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'Main guide',
          credential_id: 'credential-2',
          model: 'gpt-5',
        }),
      ),
    )
  })

  it('disables incompatible runtimes with the server-provided reason', () => {
    render(<FederatedAgentsPage />)
    fireEvent.click(screen.getByRole('button', { name: /new agent/i }))
    fireEvent.click(screen.getByRole('button', { name: /Openai · Personal ChatGPT/i }))
    const codexOption = screen
      .getByText(/OAuth handoff into the Codex CLI is not supported/)
      .closest('button') as HTMLButtonElement
    expect(codexOption.disabled).toBe(true)
  })

  it('creates a harness agent referencing a provider entry', async () => {
    render(<FederatedAgentsPage />)
    fireEvent.click(screen.getByRole('button', { name: /new agent/i }))
    const wizard = within(screen.getByRole('dialog'))
    fireEvent.click(wizard.getByRole('button', { name: /Openai · Work key/i }))
    fireEvent.click(wizard.getByRole('button', { name: /Codex CLI harness/i }))
    fireEvent.change(wizard.getByLabelText('Agent name'), { target: { value: 'Codex worker' } })
    fireEvent.click(wizard.getByRole('button', { name: /create agent/i }))
    await vi.waitFor(() =>
      expect(registerHarnessAgent).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'Codex worker',
          executor_type: 'codex',
          credential_id: 'credential-2',
        }),
      ),
    )
  })
})
