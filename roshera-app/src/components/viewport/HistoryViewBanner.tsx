/**
 * While the viewport shows a past step (the film strip or the chapter list
 * was clicked or dragged), say so plainly -- this is the part as it stood
 * then, read-only -- and offer the way back to the live part (also Esc).
 * The kernel's typed reason shows if that state could not be rebuilt.
 */
import { useEffect } from 'react'
import { useHistoryView } from '@/stores/history-view-store'

export function HistoryViewBanner() {
  const step = useHistoryView((s) => s.step)
  const label = useHistoryView((s) => s.label)
  const loading = useHistoryView((s) => s.loading)
  const error = useHistoryView((s) => s.error)
  const view = useHistoryView((s) => s.view)

  useEffect(() => {
    if (step === null) return
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
      if (e.key === 'Escape') view(null)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [step, view])

  if (step === null) return null
  return (
    <div className="absolute top-3 left-1/2 -translate-x-1/2 pointer-events-auto cad-panel px-4 py-2 flex items-center gap-3 text-[11px] uppercase tracking-wider">
      <span className="w-2 h-2 rounded-full bg-amber-500" />
      <span className="text-foreground font-semibold">
        Viewing the part after {label ? `“${label}”` : `step ${step}`}
      </span>
      <span className="text-muted-foreground">
        {error ? `could not rebuild that step — ${error}` : loading ? 'rebuilding…' : 'read-only'}
      </span>
      <button
        type="button"
        onClick={() => view(null)}
        className="ml-2 px-2 py-0.5 border border-border/60 hover:border-border text-muted-foreground hover:text-foreground transition-colors"
      >
        Roll to end
      </button>
    </div>
  )
}
