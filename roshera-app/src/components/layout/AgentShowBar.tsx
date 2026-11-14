import { Hand, Pause, Play, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { useAgentShowStore } from '@/stores/agent-show-store'

/**
 * "Claude is showing you …" -- the bar over the app while the agent drives
 * it (Varun 2026-11-14: drive, with Pause / Take over). Pause stops further
 * shows (they are counted, not lost); Take over stops, clears what was
 * shown and stays paused; × just hides the bar.
 */
export function AgentShowBar() {
  const { paused, showing, missed, pause, resume, takeOver, dismiss } = useAgentShowStore()
  if (!showing && !paused) return null
  return (
    <div
      role="status"
      aria-live="polite"
      className="pointer-events-auto absolute left-1/2 top-12 z-50 flex -translate-x-1/2 items-center gap-2 rounded-full border border-[var(--cad-selected)] bg-card px-3 py-1 text-xs shadow-lg"
    >
      <span className={cn('h-2 w-2 shrink-0 rounded-full', paused ? 'bg-muted-foreground' : 'animate-pulse bg-[var(--cad-selected)]')} />
      {paused ? (
        <span className="text-muted-foreground">
          Claude is paused{missed > 0 ? ` · ${missed} waiting` : ''}
        </span>
      ) : (
        <span className={cn('max-w-[36rem] truncate', showing?.failed && 'text-amber-700 dark:text-amber-400')}>
          <b className="font-semibold">Claude is showing you</b> {showing?.caption}
        </span>
      )}
      {paused ? (
        <button type="button" onClick={resume} className="cad-focus flex items-center gap-1 rounded-full px-2 py-0.5 hover:bg-accent" title="Let Claude drive again">
          <Play size={11} /> Resume
        </button>
      ) : (
        <>
          <button type="button" onClick={pause} className="cad-focus flex items-center gap-1 rounded-full px-2 py-0.5 hover:bg-accent" title="Stop Claude moving the app">
            <Pause size={11} /> Pause
          </button>
          <button type="button" onClick={takeOver} className="cad-focus flex items-center gap-1 rounded-full px-2 py-0.5 hover:bg-accent" title="Stop, clear what was shown, and keep control">
            <Hand size={11} /> Take over
          </button>
        </>
      )}
      <button type="button" aria-label="Hide" onClick={dismiss} className="cad-focus rounded-full p-0.5 text-muted-foreground hover:text-foreground">
        <X size={12} />
      </button>
    </div>
  )
}
