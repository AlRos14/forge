import { create } from 'zustand'
import type { AgentChat, AgentChatTurn } from '@/features/agent-chat/types'

type ChatSelection = {
  globalChatId?: string
  projectChatIds: Record<string, string | undefined>
  pendingTurns: Record<string, AgentChatTurn[]>
  setGlobalChat: (chat: AgentChat | undefined) => void
  setProjectChat: (projectId: string, chat: AgentChat | undefined) => void
  setPendingTurns: (chatId: string, turns: AgentChatTurn[]) => void
  clearPendingTurn: (chatId: string, turnId: string) => void
}

export const useChatSelection = create<ChatSelection>((set) => ({
  projectChatIds: {},
  pendingTurns: {},
  setGlobalChat: (chat) => set((current) => ({ ...current, globalChatId: chat?.id })),
  setProjectChat: (projectId, chat) =>
    set((current) => ({
      ...current,
      projectChatIds: { ...current.projectChatIds, [projectId]: chat?.id },
    })),
  setPendingTurns: (chatId, turns) =>
    set((current) => ({
      ...current,
      pendingTurns: { ...current.pendingTurns, [chatId]: turns },
    })),
  clearPendingTurn: (chatId, turnId) =>
    set((current) => {
      const turns = current.pendingTurns[chatId] ?? []
      const remaining = turns.filter((turn) => turn.id !== turnId)
      if (remaining.length === turns.length) return current
      const pendingTurns = { ...current.pendingTurns }
      if (remaining.length === 0) delete pendingTurns[chatId]
      else pendingTurns[chatId] = remaining
      return { ...current, pendingTurns }
    }),
}))
