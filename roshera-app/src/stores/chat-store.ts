import { create } from 'zustand'

export interface ChatMessage {
  id: string
  role: 'user' | 'assistant' | 'system'
  content: string
  timestamp: number
  objectsAffected?: string[]
  isError?: boolean
}

interface ChatState {
  messages: ChatMessage[]
  isProcessing: boolean
  isPanelOpen: boolean

  addMessage: (msg: Omit<ChatMessage, 'id' | 'timestamp'>) => string
  updateMessageContent: (id: string, content: string) => void
  setProcessing: (v: boolean) => void
  togglePanel: () => void
  setPanel: (open: boolean) => void
  clearMessages: () => void
}

let msgCounter = 0

export const useChatStore = create<ChatState>((set) => ({
  messages: [
    {
      id: 'welcome',
      role: 'system',
      content: 'Type a command like "create a box 10x5x3" or ask a question about your design.',
      timestamp: Date.now(),
    },
  ],
  isProcessing: false,
  isPanelOpen: true,

  addMessage: (msg) => {
    const id = `msg-${++msgCounter}`
    set((state) => ({
      messages: [
        ...state.messages,
        { ...msg, id, timestamp: Date.now() },
      ],
    }))
    return id
  },

  updateMessageContent: (id, content) =>
    set((state) => ({
      messages: state.messages.map((m) =>
        m.id === id ? { ...m, content } : m,
      ),
    })),

  setProcessing: (v) => set({ isProcessing: v }),
  togglePanel: () => set((s) => ({ isPanelOpen: !s.isPanelOpen })),
  setPanel: (open) => set({ isPanelOpen: open }),
  clearMessages: () =>
    set({
      messages: [
        {
          id: 'welcome',
          role: 'system',
          content: 'Chat cleared. Type a command to get started.',
          timestamp: Date.now(),
        },
      ],
    }),
}))
