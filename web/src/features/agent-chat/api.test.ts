import { describe, expect, it, vi } from 'vitest'
import { cancelAgentChatTurn, agentChatApiPaths } from './api'

const apiFetch = vi.hoisted(() => vi.fn())

vi.mock('@/api/client', () => ({ apiFetch }))

describe('agent chat cancel API', () => {
  it('posts the current turn version and idempotency key to the cancel route', async () => {
    const cancelledTurn = { id: 'turn-1', status: 'cancelled', version: 8n }
    apiFetch.mockResolvedValue(cancelledTurn)

    await cancelAgentChatTurn('chat-1', 'turn-1', {
      expected_version: 7,
      idempotency_key: 'cancel:turn-1:7',
    })

    expect(agentChatApiPaths.cancelTurn('chat-1', 'turn-1')).toBe(
      '/agent-chats/chat-1/turns/turn-1/cancel',
    )
    expect(apiFetch).toHaveBeenCalledWith(
      '/agent-chats/chat-1/turns/turn-1/cancel',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          expected_version: 7,
          idempotency_key: 'cancel:turn-1:7',
        }),
      }),
    )
  })
})
