import { useChatStore } from '@/stores/chat-store'
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
 * High-level: send a user message, get AI response, update chat store.
 */
export async function processUserMessage(text: string, sessionId?: string) {
  const store = useChatStore.getState()
  const wsSession = useWSStore.getState().sessionId

  store.addMessage({ role: 'user', content: text })
  store.setProcessing(true)

  try {
    const resp = await sendAICommand(text, sessionId || wsSession || undefined)

    if (resp.success && resp.result?.result) {
      const r = resp.result.result
      const message = r.message || 'Command executed.'
      const objectId = r.object_id
      store.addMessage({
        role: 'assistant',
        content: message,
        objectsAffected: objectId ? [objectId] : undefined,
      })
    } else if (resp.error) {
      store.addMessage({
        role: 'assistant',
        content: resp.error,
        isError: true,
      })
    } else {
      store.addMessage({
        role: 'assistant',
        content: 'Command processed.',
      })
    }

    // Store session ID if returned
    if (resp.session_id) {
      useWSStore.getState().setSessionId(resp.session_id)
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : 'Unknown error'
    store.addMessage({
      role: 'assistant',
      content: `Failed to reach backend: ${message}`,
      isError: true,
    })
  } finally {
    store.setProcessing(false)
  }
}
