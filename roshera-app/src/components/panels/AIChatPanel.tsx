import { useRef, useEffect, useState, useCallback } from 'react'
import { useChatStore } from '@/stores/chat-store'
import { processUserMessage } from '@/lib/ai-client'
import { ChatMessage } from './ChatMessage'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  MessageSquare,
  Send,
  Loader2,
  ChevronDown,
  Trash2,
} from 'lucide-react'

export function AIChatPanel() {
  const messages = useChatStore((s) => s.messages)
  const isProcessing = useChatStore((s) => s.isProcessing)
  const isPanelOpen = useChatStore((s) => s.isPanelOpen)
  const togglePanel = useChatStore((s) => s.togglePanel)
  const clearMessages = useChatStore((s) => s.clearMessages)

  const [input, setInput] = useState('')
  const scrollRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [messages.length])

  // Focus input when panel opens
  useEffect(() => {
    if (isPanelOpen) {
      setTimeout(() => inputRef.current?.focus(), 100)
    }
  }, [isPanelOpen])

  const handleSubmit = useCallback(
    (e?: React.FormEvent) => {
      e?.preventDefault()
      const text = input.trim()
      if (!text || isProcessing) return
      setInput('')
      processUserMessage(text)
    },
    [input, isProcessing],
  )

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        handleSubmit()
      }
    },
    [handleSubmit],
  )

  // Collapsed state — floating button
  if (!isPanelOpen) {
    return (
      <button
        onClick={togglePanel}
        className="absolute bottom-10 left-4 z-20 w-10 h-10 rounded-full bg-primary text-primary-foreground flex items-center justify-center shadow-lg hover:scale-105 transition-transform"
      >
        <MessageSquare size={18} />
      </button>
    )
  }

  return (
    <div className="absolute bottom-8 left-3 z-20 w-80 max-h-[55vh] flex flex-col rounded-xl border border-white/5 bg-transparent overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-white/5">
        <div className="flex items-center gap-2">
          <MessageSquare size={14} className="text-primary" />
          <span className="text-xs font-medium">AI Assistant</span>
        </div>
        <div className="flex items-center gap-1">
          <button
            onClick={clearMessages}
            className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
            title="Clear chat"
          >
            <Trash2 size={12} />
          </button>
          <button
            onClick={togglePanel}
            className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
            title="Minimize"
          >
            <ChevronDown size={14} />
          </button>
        </div>
      </div>

      {/* Messages */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto min-h-0 max-h-[40vh]">
        <div className="py-2">
          {messages.map((msg) => (
            <ChatMessage key={msg.id} message={msg} />
          ))}
          {isProcessing && (
            <div className="flex items-center gap-2 px-3 py-2">
              <Loader2 size={14} className="animate-spin text-primary" />
              <span className="text-xs text-muted-foreground">Thinking...</span>
            </div>
          )}
        </div>
      </div>

      {/* Input */}
      <form
        onSubmit={handleSubmit}
        className="flex items-center gap-1.5 px-2 py-2 border-t border-white/5"
      >
        <Input
          ref={inputRef}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Type a command..."
          disabled={isProcessing}
          className="h-8 text-xs bg-transparent border-white/10 placeholder:text-white/30"
        />
        <Button
          type="submit"
          size="sm"
          disabled={!input.trim() || isProcessing}
          className="h-8 w-8 p-0 shrink-0"
        >
          {isProcessing ? (
            <Loader2 size={14} className="animate-spin" />
          ) : (
            <Send size={14} />
          )}
        </Button>
      </form>
    </div>
  )
}
