import { useChatStore } from '@/stores/chat-store'
import { useBlackboardStore } from '@/stores/blackboard-store'
import { useWSStore } from '@/stores/ws-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

interface AICommandResponse {
  success: boolean
  cached?: boolean
  result?: {
    original_text: string
    command?: {
      original_text: string
      intent: Record<string, unknown>
      parameters: Record<string, unknown>
      confidence: number
    }
    result?: {
      status: string
      message: string
      object_id?: string
      properties?: Record<string, unknown>
    }
    execution_time_ms: number
  }
  error?: string
  execution_time_ms: number
  session_id?: string
}

export async function sendAICommand(
  command: string,
  sessionId?: string,
): Promise<AICommandResponse> {
  const response = await fetch(`${API_BASE}/ai/command`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      command,
      session_id: sessionId,
      use_cache: true,
    }),
  })

  if (!response.ok) {
    throw new Error(`AI command failed: ${response.status} ${response.statusText}`)
  }

  return response.json()
}

export async function sendAICommandStreaming(
  command: string,
  sessionId?: string,
  onChunk?: (content: string) => void,
): Promise<void> {
  const response = await fetch(`${API_BASE}/ai/command/stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      command,
      session_id: sessionId,
      stream_response: true,
    }),
  })

  if (!response.ok) {
    throw new Error(`AI stream failed: ${response.status}`)
  }

  const reader = response.body?.getReader()
  if (!reader) throw new Error('No response body')

  const decoder = new TextDecoder()
  let buffer = ''

  while (true) {
    const { done, value } = await reader.read()
    if (done) break

    buffer += decoder.decode(value, { stream: true })
    const lines = buffer.split('\n')
    buffer = lines.pop() || ''

    for (const line of lines) {
      if (line.startsWith('data: ')) {
        try {
          const data = JSON.parse(line.slice(6))
          if (data.content && onChunk) {
            onChunk(data.content)
          }
          if (data.result) {
            onChunk?.(data.result)
          }
        } catch {
          // non-JSON SSE line
        }
      }
    }
  }
}

/**
 * High-level: send a user message, get AI response via streaming SSE,
 * updating the chat message progressively. Falls back to non-streaming
 * if the stream endpoint is unavailable.
 */
export async function processUserMessage(text: string, sessionId?: string) {
  const store = useChatStore.getState()
  const wsSession = useWSStore.getState().sessionId
  const sid = sessionId || wsSession || undefined

  store.addMessage({ role: 'user', content: text })
  store.setProcessing(true)

  // Create a placeholder assistant message for progressive updates
  const msgId = store.addMessage({ role: 'assistant', content: '' })
  let accumulated = ''

  try {
    await sendAICommandStreaming(text, sid, (chunk) => {
      accumulated += chunk
      useChatStore.getState().updateMessageContent(msgId, accumulated)
    })

    // If streaming completed but no content arrived, show fallback text
    if (!accumulated) {
      useChatStore.getState().updateMessageContent(msgId, 'Command processed.')
    }
  } catch {
    // Stream endpoint unavailable — fall back to blocking request
    try {
      const resp = await sendAICommand(text, sid)

      if (resp.success && resp.result?.result) {
        const r = resp.result.result
        const message = r.message || 'Command executed.'
        useChatStore.getState().updateMessageContent(msgId, message)

        if (r.object_id) {
          // Update the message with affected objects metadata
          const current = useChatStore.getState().messages.find((m) => m.id === msgId)
          if (current) {
            useChatStore.setState((state) => ({
              messages: state.messages.map((m) =>
                m.id === msgId
                  ? { ...m, objectsAffected: [r.object_id!] }
                  : m,
              ),
            }))
          }
        }
      } else if (resp.error) {
        useChatStore.setState((state) => ({
          messages: state.messages.map((m) =>
            m.id === msgId
              ? { ...m, content: resp.error!, isError: true }
              : m,
          ),
        }))
      } else {
        useChatStore.getState().updateMessageContent(msgId, 'Command processed.')
      }

      if (resp.session_id) {
        useWSStore.getState().setSessionId(resp.session_id)
      }
    } catch (fallbackErr) {
      const message = fallbackErr instanceof Error ? fallbackErr.message : 'Unknown error'
      useChatStore.setState((state) => ({
        messages: state.messages.map((m) =>
          m.id === msgId
            ? { ...m, content: `Failed to reach backend: ${message}`, isError: true }
            : m,
        ),
      }))
    }
  } finally {
    store.setProcessing(false)
  }
}

/**
 * Blackboard variant of `processUserMessage`. Same agent plumbing
 * (`sendAICommandStreaming` → `sendAICommand` fallback), but the user prompt
 * and the agent reply are appended to the Blackboard as *editable lines*
 * rather than chat bubbles.
 *
 * Streaming chunks update the agent line in place via `setLineText` (no event
 * spam); once the reply settles we commit a single `edit` event through
 * `editLine` so the append-only log records exactly one meaningful entry for
 * the final agent text. The initial `addLine` already logged the line's
 * creation, so reload + history-scrub both see a coherent sequence.
 */
export async function processBlackboardMessage(text: string, sessionId?: string) {
  const board = useBlackboardStore.getState()
  const wsSession = useWSStore.getState().sessionId
  const sid = sessionId || wsSession || undefined

  board.addLine(text, 'user')
  board.setProcessing(true)

  // Placeholder agent line for progressive streaming. Created empty so the
  // `add` event is logged immediately; final text is committed via `editLine`.
  const lineId = board.addLine('', 'agent')
  let accumulated = ''

  const commit = (content: string) => {
    useBlackboardStore.getState().editLine(lineId, content)
  }

  try {
    await sendAICommandStreaming(text, sid, (chunk) => {
      accumulated += chunk
      useBlackboardStore.getState().setLineText(lineId, accumulated)
    })
    commit(accumulated || 'Command processed.')
  } catch {
    try {
      const resp = await sendAICommand(text, sid)
      if (resp.success && resp.result?.result) {
        commit(resp.result.result.message || 'Command executed.')
      } else if (resp.error) {
        commit(resp.error)
      } else {
        commit('Command processed.')
      }
      if (resp.session_id) {
        useWSStore.getState().setSessionId(resp.session_id)
      }
    } catch (fallbackErr) {
      const message = fallbackErr instanceof Error ? fallbackErr.message : 'Unknown error'
      commit(`Failed to reach backend: ${message}`)
    }
  } finally {
    useBlackboardStore.getState().setProcessing(false)
  }
}
