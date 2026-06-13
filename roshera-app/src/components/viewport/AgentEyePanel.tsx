import { useCallback, useEffect, useRef, useState } from 'react'

const API_HOST = import.meta.env.VITE_API_URL || ''

type RenderMode = 'shaded' | 'diagnostic' | 'ids'
type ViewName = 'iso' | 'front' | 'top' | 'right'

interface RenderResponse {
  png_base64: string
  open_edges: number
  nonmanifold_edges: number
}

/**
 * Agent-Eye panel — a small, minimizable, LIVE window showing exactly what
 * the agent/kernel "sees": the server-side deterministic render of the
 * newest part (`GET /api/agent/parts/{id}/render`). This is the human-facing
 * mirror of the agent's perception loop. `diagnostic` mode overlays open
 * (red) / non-manifold (magenta) edges so watertightness is visible at a
 * glance; `ids` paints each B-Rep face a distinct flat colour.
 *
 * It polls the same endpoint the agent reads, so what you watch here is the
 * exact frame the agent grabs when it inspects geometry.
 */
export function AgentEyePanel() {
  const [minimized, setMinimized] = useState(false)
  const [live, setLive] = useState(true)
  const [mode, setMode] = useState<RenderMode>('shaded')
  const [view, setView] = useState<ViewName>('iso')
  const [png, setPng] = useState<string | null>(null)
  const [diag, setDiag] = useState<{ open: number; nm: number } | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const inFlight = useRef(false)

  const grab = useCallback(async () => {
    if (inFlight.current) return
    inFlight.current = true
    try {
      const partsR = await fetch(`${API_HOST}/api/agent/parts`)
      if (!partsR.ok) throw new Error(`parts ${partsR.status}`)
      const parts = (await partsR.json()) as Array<{ id: number }>
      if (!Array.isArray(parts) || parts.length === 0) {
        setPng(null)
        setDiag(null)
        setErr('no geometry yet')
        return
      }
      const id = parts.reduce((m, p) => Math.max(m, p.id), 0)
      const r = await fetch(
        `${API_HOST}/api/agent/parts/${id}/render?mode=${mode}&view=${view}&size=256`,
      )
      if (!r.ok) throw new Error(`render ${r.status}`)
      const data = (await r.json()) as RenderResponse
      setPng(data.png_base64)
      setDiag({ open: data.open_edges, nm: data.nonmanifold_edges })
      setErr(null)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      inFlight.current = false
    }
  }, [mode, view])

  useEffect(() => {
    if (minimized) return
    grab()
    if (!live) return
    const t = setInterval(grab, 1200)
    return () => clearInterval(t)
  }, [minimized, live, grab])

  if (minimized) {
    return (
      <button
        onClick={() => setMinimized(false)}
        className="absolute bottom-2 right-2 z-20 rounded-md border border-border bg-background/90 px-3 py-1.5 text-xs font-medium shadow-md backdrop-blur hover:bg-accent"
        title="Open the agent-eye view"
      >
        👁 Agent Eye
      </button>
    )
  }

  return (
    <div className="absolute bottom-2 right-2 z-20 w-[208px] overflow-hidden rounded-md border border-border bg-background/95 shadow-lg backdrop-blur">
      <div className="flex items-center justify-between border-b border-border px-2 py-1.5">
        <div className="flex items-center gap-1.5 text-xs font-semibold">
          <span>👁 Agent Eye</span>
          <span
            className={`inline-block h-1.5 w-1.5 rounded-full ${
              live ? 'animate-pulse bg-green-500' : 'bg-muted-foreground'
            }`}
          />
          <span className="text-[10px] text-muted-foreground">{live ? 'LIVE' : 'paused'}</span>
        </div>
        <div className="flex items-center gap-1">
          <button
            onClick={() => setLive((v) => !v)}
            className="rounded px-1.5 py-0.5 text-[10px] hover:bg-accent"
            title={live ? 'Pause live updates' : 'Resume live updates'}
          >
            {live ? '⏸' : '▶'}
          </button>
          <button
            onClick={grab}
            className="rounded px-1.5 py-0.5 text-[10px] hover:bg-accent"
            title="Refresh now"
          >
            ⟳
          </button>
          <button
            onClick={() => setMinimized(true)}
            className="rounded px-1.5 py-0.5 text-[10px] hover:bg-accent"
            title="Minimize"
          >
            ▁
          </button>
        </div>
      </div>

      <div className="relative aspect-square w-full bg-muted">
        {png ? (
          <img
            src={`data:image/png;base64,${png}`}
            alt="agent view"
            className="h-full w-full object-contain"
          />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-xs text-muted-foreground">
            {err ?? 'loading…'}
          </div>
        )}
        {mode === 'diagnostic' && diag && (
          <div className="absolute left-1 top-1 rounded bg-background/80 px-1.5 py-0.5 font-mono text-[10px]">
            <span className={diag.open ? 'text-red-500' : 'text-green-500'}>open {diag.open}</span>
            {' · '}
            <span className={diag.nm ? 'text-fuchsia-500' : 'text-green-500'}>nm {diag.nm}</span>
          </div>
        )}
      </div>

      <div className="flex items-center justify-between gap-1 border-t border-border px-2 py-1">
        <div className="flex gap-0.5">
          {(['shaded', 'diagnostic', 'ids'] as RenderMode[]).map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              className={`rounded px-1.5 py-0.5 text-[10px] ${
                mode === m ? 'bg-primary text-primary-foreground' : 'hover:bg-accent'
              }`}
            >
              {m === 'diagnostic' ? 'diag' : m}
            </button>
          ))}
        </div>
        <div className="flex gap-0.5">
          {(['iso', 'front', 'top', 'right'] as ViewName[]).map((v) => (
            <button
              key={v}
              onClick={() => setView(v)}
              className={`rounded px-1.5 py-0.5 text-[10px] ${
                view === v ? 'bg-primary text-primary-foreground' : 'hover:bg-accent'
              }`}
            >
              {v}
            </button>
          ))}
        </div>
      </div>
    </div>
  )
}
