import { useCallback, useEffect, useRef, useState } from 'react'

const API_HOST = import.meta.env.VITE_API_URL || ''

// Single-view render modes hit /render; `dim` and `section` hit the richer
// EYE-1 / EYE-2 endpoints (2×2 dimensioned composite and a cross-section).
type RenderMode = 'shaded' | 'diagnostic' | 'ids' | 'dim' | 'section'
type ViewName = 'iso' | 'front' | 'top' | 'right'

const SINGLE_VIEW: RenderMode[] = ['shaded', 'diagnostic', 'ids']

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
  const [perc, setPerc] = useState<{
    watertight: boolean
    open_edges: number
    nonmanifold_edges: number
    valid: boolean
    dims: number[] | null
  } | null>(null)
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
      // Feedback-as-default: the part's soundness, always shown, every poll.
      try {
        const pr = await fetch(`${API_HOST}/api/agent/parts/${id}/perception`)
        if (pr.ok) setPerc(await pr.json())
      } catch {
        /* perception is best-effort; never block the render */
      }
      if (mode === 'dim') {
        // EYE-1: 2×2 dimensioned multiview (triad, scale bar, L×W×H, centroid).
        const r = await fetch(`${API_HOST}/api/agent/parts/${id}/dimensioned`)
        if (!r.ok) throw new Error(`dimensioned ${r.status}`)
        const data = await r.json()
        setPng(data.png_base64)
        setDiag(null)
        setErr(null)
      } else if (mode === 'section') {
        // EYE-2: cut through the part's bbox centre (so it works off-origin).
        const dr = await fetch(`${API_HOST}/api/agent/parts/${id}/dimensioned`)
        if (!dr.ok) throw new Error(`dim ${dr.status}`)
        const d = await dr.json()
        const cx = (d.bbox_min.x + d.bbox_max.x) / 2
        const cy = (d.bbox_min.y + d.bbox_max.y) / 2
        const cz = (d.bbox_min.z + d.bbox_max.z) / 2
        const sr = await fetch(
          `${API_HOST}/api/agent/parts/${id}/section?px=${cx}&py=${cy}&pz=${cz}&nx=0&ny=0&nz=1`,
        )
        if (!sr.ok) throw new Error(`section ${sr.status}`)
        const data = await sr.json()
        setPng(data.png_base64)
        setDiag(null)
        setErr(null)
      } else {
        const r = await fetch(
          `${API_HOST}/api/agent/parts/${id}/render?mode=${mode}&view=${view}&size=256`,
        )
        if (!r.ok) throw new Error(`render ${r.status}`)
        const data = (await r.json()) as { png_base64: string; open_edges: number; nonmanifold_edges: number }
        setPng(data.png_base64)
        setDiag({ open: data.open_edges, nm: data.nonmanifold_edges })
        setErr(null)
      }
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

      {perc && (
        <div className="flex items-center justify-between gap-2 border-t border-border px-2 py-1 font-mono text-[10px]">
          <span className={perc.watertight && perc.valid ? 'text-green-600' : 'text-red-500'}>
            {perc.watertight && perc.valid ? '✓ sound' : '✗ defect'}
          </span>
          <span className={perc.open_edges ? 'text-red-500' : 'text-muted-foreground'}>
            open {perc.open_edges}
          </span>
          <span className={perc.nonmanifold_edges ? 'text-fuchsia-500' : 'text-muted-foreground'}>
            nm {perc.nonmanifold_edges}
          </span>
          <span className={perc.valid ? 'text-muted-foreground' : 'text-red-500'}>
            {perc.valid ? 'valid' : 'invalid'}
          </span>
          {perc.dims && (
            <span className="text-muted-foreground">
              {perc.dims.map((d) => Math.round(d)).join('×')}
            </span>
          )}
        </div>
      )}

      <div className="flex items-center justify-between gap-1 border-t border-border px-2 py-1">
        <div className="flex gap-0.5">
          {(['shaded', 'diagnostic', 'ids', 'dim', 'section'] as RenderMode[]).map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              className={`rounded px-1 py-0.5 text-[10px] ${
                mode === m ? 'bg-primary text-primary-foreground' : 'hover:bg-accent'
              }`}
            >
              {m === 'diagnostic' ? 'diag' : m === 'section' ? 'sec' : m}
            </button>
          ))}
        </div>
        {SINGLE_VIEW.includes(mode) && (
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
        )}
      </div>
    </div>
  )
}
