import { useRef, useEffect, useState, useCallback } from 'react'
import { useBlackboardStore } from '@/stores/blackboard-store'
import { processBlackboardMessage } from '@/lib/ai-client'
import { BlackboardLine } from './BlackboardLine'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  NotebookPen,
  Send,
  Loader2,
  ChevronDown,
  Plus,
  Trash2,
} from 'lucide-react'

/**
 * BLACKBOARD
 * ==========
 * A wide, editable, logged document that supersedes the chat-transcript UX.
 * The body is a document of independently-editable lines (no message bubbles).
 * The agent writes replies as editable lines; the user can edit any line and
 * add their own. Every create/edit/delete is event-sourced + persisted by the
 * blackboard store (localStorage today, backend-swappable seam).
 *
 * Width note: the legacy AI chat panel was `w-80` (20rem). Per spec this panel
 * is 2.5× as wide → `w-[50rem]` (50rem / 800px), capped to the viewport so the
 * 3D scene stays usable on narrow screens.
 */
export function Blackboard() {
  const lines = useBlackboardStore((s) => s.lines)
  const isProcessing = useBlackboardStore((s) => s.isProcessing)
  const isPanelOpen = useBlackboardStore((s) => s.isPanelOpen)
  const togglePanel = useBlackboardStore((s) => s.togglePanel)
  const clearBoard = useBlackboardStore((s) => s.clearBoard)
  const editLine = useBlackboardStore((s) => s.editLine)
  const deleteLine = useBlackboardStore((s) => s.deleteLine)
  const addLine = useBlackboardStore((s) => s.addLine)

  const [input, setInput] = useState('')
  const scrollRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // Auto-scroll to the newest line as the document grows.
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [lines.length])

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
      // Routes to the agent via the existing ai-client path; the reply is
      // appended to the board as an editable line, not a chat bubble.
      void processBlackboardMessage(text)
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

  // Collapsed state — floating button.
  if (!isPanelOpen) {
    return (
      <button
        onClick={togglePanel}
        className="cad-focus absolute bottom-10 left-4 z-20 w-10 h-10 rounded-full bg-primary text-primary-foreground flex items-center justify-center shadow-lg hover:scale-105 transition-transform"
        aria-label="Open Blackboard"
        title="Blackboard"
      >
        <NotebookPen size={18} />
      </button>
    )
  }

  return (
    <div className="absolute bottom-8 left-3 z-20 w-[50rem] max-w-[calc(100vw-1.5rem)] max-h-[60vh] flex flex-col rounded-xl overflow-hidden bg-background/35 backdrop-blur-md border border-border/60">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-white/5">
        <div className="flex items-center gap-2">
          <NotebookPen size={14} className="text-primary" />
          <span className="text-xs font-medium">Blackboard</span>
        </div>
        <div className="flex items-center gap-0.5">
          <button
            onClick={() => addLine('', 'user')}
            className="cad-icon-btn h-6 w-6"
            title="Add line"
            aria-label="Add line"
          >
            <Plus size={13} />
          </button>
          <button
            onClick={clearBoard}
            className="cad-icon-btn h-6 w-6"
            title="Clear board"
            aria-label="Clear board"
          >
            <Trash2 size={12} />
          </button>
          <button
            onClick={togglePanel}
            className="cad-icon-btn h-6 w-6"
            title="Minimize"
            aria-label="Minimize Blackboard"
          >
            <ChevronDown size={14} />
          </button>
        </div>
      </div>

      {/* Document of editable lines */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto min-h-0 max-h-[45vh]">
        <div className="py-2">
          {lines.map((line) => (
            <BlackboardLine
              key={line.id}
              line={line}
              onCommit={editLine}
              onDelete={deleteLine}
            />
          ))}
          {isProcessing && (
            <div className="flex items-center gap-2 px-3 py-2">
              <Loader2 size={14} className="animate-spin text-primary" />
              <span className="text-xs text-muted-foreground">Thinking...</span>
            </div>
          )}
        </div>
      </div>

      {/* Prompt — still routes to the agent */}
      <form
        onSubmit={handleSubmit}
        className="flex items-center gap-1.5 px-2 py-2 border-t border-white/5"
      >
        <Input
          ref={inputRef}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Ask the agent — its reply lands as an editable line…"
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
