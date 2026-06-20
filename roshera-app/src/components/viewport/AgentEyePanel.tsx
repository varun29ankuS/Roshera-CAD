import { useCallback, useEffect, useRef, useState } from 'react'

const API_HOST = import.meta.env.VITE_API_URL || ''

// Single-view render modes hit /render; `dim` and `section` hit the richer
// EYE-1 / EYE-2 endpoints (2×2 dimensioned composite and a cross-section).
type RenderMode = 'shaded' | 'diagnostic' | 'ids' | 'dim' | 'section'
type ViewName = 'iso' | 'front' | 'top' | 'right'

// What the eye is looking at: a single part (newest/selected), the whole
// scene (every solid composited via the scene-eye), or one NAMED instanced
// assembly (#19 — every instance composited at its transform via
// `/api/assembly/{id}/view`).
type Scope = 'part' | 'assembly' | 'named'

const SINGLE_VIEW: RenderMode[] = ['shaded', 'diagnostic', 'ids']

// The scene-eye (`/api/agent/scene/orbit`) accepts the same render mode names
// but has no per-part `dim`/`section` notion. These are the modes shared by
// both endpoints, so the mode selector stays meaningful when toggling scope.
const SCENE_MODES: RenderMode[] = ['shaded', 'diagnostic', 'ids']

const DEFAULT_AZ = 35
const DEFAULT_EL = 20

/**
 * Agent-Eye panel — a small, minimizable, LIVE window showing exactly what
 * the agent/kernel "sees". Two scopes:
 *
 *  - **Part** (`GET /api/agent/parts/{id}/render` + the EYE-1/EYE-2
 *    `/dimensioned` and `/section` endpoints): the newest part on its own,
 *    with dimensioned multiview and cross-section.
 *  - **Assembly** (`GET /api/agent/scene/orbit?az&el&mode&size&quality`): every
 *    solid composited into one auto-framed frame, orbitable by az/el. This is
 *    the same composite the MCP `scene_view` tool reads — so the human sees the
 *    full assembly the agent drives, not just one part.
 *
 * `diagnostic` mode overlays open (red) / non-manifold (magenta) edges so
 * watertightness is visible at a glance; `ids` paints each B-Rep face a
 * distinct flat colour. The panel polls the same endpoints the agent reads.
 */
export function AgentEyePanel() {
  const [minimized, setMinimized] = useState(false)
  const [live, setLive] = useState(true)
  const [scope, setScope] = useState<Scope>('part')
  // The instanced-assembly id the 'named' scope renders (#19). Empty = the
  // user hasn't picked one yet; the panel prompts for it.
  const [assemblyId, setAssemblyId] = useState('')
  const [mode, setMode] = useState<RenderMode>('shaded')
  const [view, setView] = useState<ViewName>('iso')
  const [az, setAz] = useState(DEFAULT_AZ)
  const [el, setEl] = useState(DEFAULT_EL)
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

  // In Assembly / named-assembly mode only the scene-shared modes are valid;
  // fall back cleanly if the user was on `dim`/`section` when they switched.
  const sceneMode: RenderMode = SCENE_MODES.includes(mode) ? mode : 'shaded'
  // Scopes that render a composited scene (orbit camera + scene modes).
  const isSceneScope = scope === 'assembly' || scope === 'named'

  const grab = useCallback(async () => {
    if (inFlight.current) return
    inFlight.current = true
    try {
      if (scope === 'named') {
        // NAMED-ASSEMBLY EYE (#19): composite every instance of one assembly
        // at its transform. Per-part perception is not meaningful here; the
        // assembly's own perception comes from GET /api/assembly/{id}.
        setPerc(null)
        if (!assemblyId.trim()) {
          setPng(null)
          setDiag(null)
          setErr('enter an assembly id')
          return
        }
        const r = await fetch(
          `${API_HOST}/api/assembly/${assemblyId.trim()}/view?az=${az}&el=${el}&mode=${sceneMode}&size=256&quality=medium`,
        )
        if (r.status === 404) {
          setPng(null)
          setDiag(null)
          setErr('no such assembly / no instances')
          return
        }
        if (!r.ok) throw new Error(`assembly ${r.status}`)
        const data = (await r.json()) as {
          png_base64: string
          open_edges: number
          nonmanifold_edges: number
        }
        setPng(data.png_base64)
        setDiag({ open: data.open_edges, nm: data.nonmanifold_edges })
        setErr(null)
        return
      }

      if (scope === 'assembly') {
        // SCENE-EYE: composite every solid, auto-framed, orbitable by az/el.
        // Per-part perception (`perc`) is not meaningful for a whole scene.
        setPerc(null)
        const r = await fetch(
          `${API_HOST}/api/agent/scene/orbit?az=${az}&el=${el}&mode=${sceneMode}&size=256&quality=medium`,
        )
        if (r.status === 404) {
          setPng(null)
          setDiag(null)
          setErr('no geometry yet')
          return
        }
        if (!r.ok) throw new Error(`scene ${r.status}`)
        const data = (await r.json()) as {
          png_base64: string
          open_edges: number
          nonmanifold_edges: number
        }
        setPng(data.png_base64)
        setDiag({ open: data.open_edges, nm: data.nonmanifold_edges })
        setErr(null)
        return
      }

      const partsR = await fetch(`${API_HOST}/api/agent/parts`)
      if (!partsR.ok) throw new Error(`parts ${partsR.status}`)
      const parts = (await partsR.json()) as Array<{ id: number }>
      if (!Array.isArray(parts) || parts.length === 0) {
        setPng(null)
        setDiag(null)
        setPerc(null)
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
  }, [scope, mode, view, az, el, sceneMode, assemblyId])

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

  const activeMode: RenderMode = isSceneScope ? sceneMode : mode
  const modeOptions: RenderMode[] =
    isSceneScope ? SCENE_MODES : (['shaded', 'diagnostic', 'ids', 'dim', 'section'] as RenderMode[])

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

      {/* Scope: a single part vs the whole assembly (scene-eye composite). */}
      <div className="flex border-b border-border">
        {(['part', 'assembly', 'named'] as Scope[]).map((s) => (
          <button
            key={s}
            onClick={() => setScope(s)}
            className={`flex-1 px-2 py-1 text-[10px] font-medium capitalize ${
              scope === s
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:bg-accent'
            }`}
            title={
              s === 'part'
                ? 'Show the newest part on its own'
                : s === 'assembly'
                  ? 'Show the whole scene (every solid composited)'
                  : 'Show one named instanced assembly (every instance at its transform)'
            }
          >
            {s === 'named' ? 'asm' : s}
          </button>
        ))}
      </div>

      {/* Named-assembly id input (#19): which instanced assembly to render. */}
      {scope === 'named' && (
        <div className="border-b border-border px-2 py-1">
          <input
            type="text"
            value={assemblyId}
            onChange={(e) => setAssemblyId(e.target.value)}
            placeholder="assembly id (uuid)"
            className="w-full rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[10px]"
          />
        </div>
      )}

      <div className="relative aspect-square w-full bg-muted">
        {png ? (
          <img
            src={`data:image/png;base64,${png}`}
            alt={isSceneScope ? 'agent scene view' : 'agent part view'}
            className="h-full w-full object-contain"
          />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-xs text-muted-foreground">
            {err ?? 'loading…'}
          </div>
        )}
        {activeMode === 'diagnostic' && diag && (
          <div className="absolute left-1 top-1 rounded bg-background/80 px-1.5 py-0.5 font-mono text-[10px]">
            <span className={diag.open ? 'text-red-500' : 'text-green-500'}>open {diag.open}</span>
            {' · '}
            <span className={diag.nm ? 'text-fuchsia-500' : 'text-green-500'}>nm {diag.nm}</span>
          </div>
        )}
      </div>

      {scope === 'part' && perc && (
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
          {modeOptions.map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              className={`rounded px-1 py-0.5 text-[10px] ${
                activeMode === m ? 'bg-primary text-primary-foreground' : 'hover:bg-accent'
              }`}
            >
              {m === 'diagnostic' ? 'diag' : m === 'section' ? 'sec' : m}
            </button>
          ))}
        </div>
        {scope === 'part' && SINGLE_VIEW.includes(mode) && (
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

      {/* Scene orbit controls: az/el step the scene-eye camera. */}
      {isSceneScope && (
        <div className="flex items-center justify-between gap-1 border-t border-border px-2 py-1 font-mono text-[10px]">
          <div className="flex items-center gap-0.5">
            <span className="text-muted-foreground">az</span>
            <button
              onClick={() => setAz((a) => a - 15)}
              className="rounded px-1 py-0.5 hover:bg-accent"
              title="Orbit left"
            >
              −
            </button>
            <span className="w-7 text-center tabular-nums">{Math.round(az)}°</span>
            <button
              onClick={() => setAz((a) => a + 15)}
              className="rounded px-1 py-0.5 hover:bg-accent"
              title="Orbit right"
            >
              +
            </button>
          </div>
          <div className="flex items-center gap-0.5">
            <span className="text-muted-foreground">el</span>
            <button
              onClick={() => setEl((e) => Math.max(-89, e - 15))}
              className="rounded px-1 py-0.5 hover:bg-accent"
              title="Tilt down"
            >
              −
            </button>
            <span className="w-7 text-center tabular-nums">{Math.round(el)}°</span>
            <button
              onClick={() => setEl((e) => Math.min(89, e + 15))}
              className="rounded px-1 py-0.5 hover:bg-accent"
              title="Tilt up"
            >
              +
            </button>
          </div>
          <button
            onClick={() => {
              setAz(DEFAULT_AZ)
              setEl(DEFAULT_EL)
            }}
            className="rounded px-1.5 py-0.5 hover:bg-accent"
            title="Reset orbit"
          >
            ⌂
          </button>
        </div>
      )}
    </div>
  )
}
