import { useCallback, useEffect, useRef, useState } from 'react'
import { ModeChip, ModeChipGroup, StatusPill, TabChip } from '@/components/ui/status-pill'

const API_HOST = import.meta.env.VITE_API_URL || ''

// Single-view render modes hit /render; `dim` and `section` hit the richer
// EYE-1 / EYE-2 endpoints (2×2 dimensioned composite and a cross-section).
type RenderMode = 'shaded' | 'diagnostic' | 'ids' | 'dim' | 'section'
type ViewName = 'iso' | 'front' | 'top' | 'right'

// What the eye is looking at: a single part (newest/selected), or an
// ASSEMBLY. The assembly scope renders a named instanced assembly (#19) by id
// via `/api/assembly/{id}/view`, and falls back to the whole-scene composite
// (`/api/agent/scene/orbit`) when no id is given — i.e. the whole scene is the
// implicit "everything" assembly. Two scopes only: part + assembly.
type Scope = 'part' | 'assembly'

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
/**
 * `onMinimizedChange` lets the rail know whether this panel is showing.
 *
 * The rail's splitter sets an explicit height on the slot below it, and an
 * explicit height on a collapsed panel is a tall empty card. The rail cannot
 * ask the DOM mid-render, so the panel reports it.
 */
export function AgentEyePanel({
  onMinimizedChange,
}: {
  onMinimizedChange?: (minimized: boolean) => void
} = {}) {
  const [minimized, setMinimizedState] = useState(false)
  const setMinimized = useCallback(
    (next: boolean) => {
      setMinimizedState(next)
      onMinimizedChange?.(next)
    },
    [onMinimizedChange],
  )
  const [live, setLive] = useState(true)
  const [scope, setScope] = useState<Scope>('part')
  // The instanced-assembly id the 'assembly' scope renders (#19). Empty = render
  // the whole-scene composite instead (the implicit "everything" assembly).
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

  // In Assembly mode only the scene-shared modes are valid; fall back cleanly
  // if the user was on `dim`/`section` when they switched.
  const sceneMode: RenderMode = SCENE_MODES.includes(mode) ? mode : 'shaded'
  // The assembly scope renders a composited scene (orbit camera + scene modes).
  const isSceneScope = scope === 'assembly'

  const grab = useCallback(async () => {
    if (inFlight.current) return
    inFlight.current = true
    try {
      if (scope === 'assembly') {
        // ASSEMBLY EYE: a named instanced assembly (#19) when an id is given —
        // every instance composited at its transform via /api/assembly/{id}/view;
        // otherwise the whole-scene composite (/api/agent/scene/orbit), i.e. the
        // implicit "everything" assembly. Per-part perception isn't meaningful for
        // a composite (a named assembly's own perception is GET /api/assembly/{id}).
        setPerc(null)
        const named = assemblyId.trim()
        const url = named
          ? `${API_HOST}/api/assembly/${named}/view?az=${az}&el=${el}&mode=${sceneMode}&size=256&quality=medium`
          : `${API_HOST}/api/agent/scene/orbit?az=${az}&el=${el}&mode=${sceneMode}&size=256&quality=medium`
        const r = await fetch(url)
        if (r.status === 404) {
          setPng(null)
          setDiag(null)
          setErr(named ? 'no such assembly / no instances' : 'no geometry yet')
          return
        }
        if (!r.ok) throw new Error(`${named ? 'assembly' : 'scene'} ${r.status}`)
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
      // mt-auto, because the dock is a flex column and this bar is sometimes
      // its only child. With a selection, Properties grows and pushes the bar
      // down on its own; with nothing selected there is nothing above it, and
      // without mt-auto the bar strands itself at the TOP of a full-height
      // empty card — a top border drawn against no edge, with the rest of the
      // column blank beneath it. It belongs at the foot either way.
      <button
        onClick={() => setMinimized(false)}
        className="mt-auto w-full shrink-0 border-t border-border bg-card px-3 py-1.5 text-left text-xs font-medium hover:bg-accent"
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
    // DOCKED, not floating. A panel parked permanently over the canvas occludes
    // the model in an app whose whole purpose is looking at the model, and it
    // took its own width, border and shadow with it. The dock owns the chrome
    // now; this fills the slot it is given.
    // shrink-0, not flex-1. Agent Eye and the Blackboard both claimed flex-1
    // and split the column evenly, which squeezed a transcript down to six
    // lines to give a fixed-size thumbnail half the rail. The board is the
    // tenant that grows; this one is sized by its content and sits at the foot.
    <div className="flex w-full shrink-0 flex-col overflow-hidden">
      {/* Hug. The rule was "roles bookend", and bookending a title against its
          own three buttons was defensible at 224px where the seam was ~18px —
          at 560 it is a 332px hole in a panel header. The rule was written for
          full-width app BARS, where end-anchoring spans real working space; a
          panel header is one unit and should read as one.
          Amended: bookend app-level bars, panel headers hug.
          The trailing space after the last button is row slack, not a void —
          every left-aligned toolbar has it. */}
      <div className="flex items-center gap-3 border-b border-border px-2 py-1.5">
        <div className="flex min-w-0 items-center gap-1.5 text-xs font-semibold">
          {/* nowrap: the header is 208px wide and the status pill grew with the
              11px type floor, which wrapped the title to "Agent / Eye". A panel
              title that reflows because a sibling changed size is a layout that
              was only ever accidentally correct. */}
          {/* The eye glyph is gone. The header is ~206px of content in 208px
              at the narrow width, so the wordmark — the only element allowed
              to give — was the one that gave, and "Agen…" is a worse panel
              title than no decoration. Dropping the glyph frees ~19px, which
              is the whole margin. It carries an ellipsis and a title anyway,
              so any future cut is still admitted twice over. */}
          <span className="truncate" title="Agent Eye">
            Agent Eye
          </span>
          {/* Liveness is a verdict about the machine, so it wears the one
              status grammar rather than a bespoke dot-and-word pair. */}
          <StatusPill tone={live ? 'positive' : 'neutral'} label={live ? 'LIVE' : 'paused'} />
        </div>
        <div className="flex shrink-0 items-center gap-0.5">
          <button
            onClick={() => setLive((v) => !v)}
            className="rounded px-1.5 py-0.5 text-[11px] hover:bg-accent"
            title={live ? 'Pause live updates' : 'Resume live updates'}
          >
            {live ? '⏸' : '▶'}
          </button>
          <button
            onClick={grab}
            className="rounded px-1.5 py-0.5 text-[11px] hover:bg-accent"
            title="Refresh now"
          >
            ⟳
          </button>
          <button
            onClick={() => setMinimized(true)}
            className="rounded px-1.5 py-0.5 text-[11px] hover:bg-accent"
            title="Minimize"
          >
            ▁
          </button>
        </div>
      </div>

      {/* Scope: a single part vs an assembly (a named instanced assembly by id,
          or the whole-scene composite when no id is given). */}
      {/* Peers hug. `flex-1` on each chip made 'Part' and 'Assembly' into two
          280px slabs at the working width — a tab is a label, not a column, and
          a tab row ending short of the right edge is correct. */}
      <div className="flex items-center gap-1 border-b border-border px-2 py-1">
        {(['part', 'assembly'] as Scope[]).map((s) => (
          <TabChip
            key={s}
            selected={scope === s}
            onClick={() => setScope(s)}
            className="whitespace-nowrap px-2.5 py-1 text-[11px] font-medium capitalize"
            title={
              s === 'part'
                ? 'Show the newest part on its own'
                : 'Show an assembly — enter an id for a named instanced assembly, or leave blank for the whole scene'
            }
          >
            {s}
          </TabChip>
        ))}
      </div>

      {/* Assembly id input (#19): a named instanced assembly to render; blank
          renders the whole-scene composite (the implicit "everything" assembly). */}
      {scope === 'assembly' && (
        <div className="border-b border-border px-2 py-1">
          <input
            type="text"
            value={assemblyId}
            onChange={(e) => setAssemblyId(e.target.value)}
            placeholder="assembly id (uuid) — blank = whole scene"
            className="w-full rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[11px]"
          />
        </div>
      )}

      {/* The render and the kernel's verdict share one row.
          `aspect-square w-full max-w-[240px] mx-auto` put a 240px image in the
          middle of a 560px field with 160px of dead grey either side — panel
          slack dumped symmetrically around an island. The cap was innocent;
          the centring was the mistake. The image is anchored left at a fixed
          200px and the slack goes to the readout, which is where the eye should
          be going anyway.

          One tree, both widths, no container queries: at 560 the render and the
          readout sit side by side and there is no grey field at all; at 224 the
          row cannot fit both, so flex-wrap stacks them and the readout becomes
          a spec table under the image. `object-contain` with the background on
          the image itself means an off-square snapshot letterboxes INSIDE its
          own frame, so no field ever surrounds it. */}
      <div className="flex flex-wrap items-start gap-3 px-2 py-2">
        <div className="relative h-[200px] w-[200px] shrink-0 overflow-hidden rounded border border-border bg-muted">
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
            <div className="absolute left-1 top-1 rounded bg-background/80 px-1.5 py-0.5 font-mono text-[11px]">
              <span className={diag.open ? 'text-red-500' : 'text-green-500'}>open {diag.open}</span>
              {' · '}
              <span className={diag.nm ? 'text-fuchsia-500' : 'text-green-500'}>nm {diag.nm}</span>
            </div>
          )}
        </div>

        {scope === 'part' && perc && (
          // Label/value pairs in a column, not five chips distributed across
          // the panel. Bookending is legal at the PAIR level — that is a spec
          // sheet — and illegal across five peers, which is what made the old
          // strip read as scattered.
          <dl className="flex min-w-[150px] flex-1 flex-col gap-1 font-mono text-[11px]">
            <div className="flex justify-between gap-3">
              <dt className="text-muted-foreground">soundness</dt>
              <dd className={perc.watertight && perc.valid ? 'text-green-600' : 'text-red-500'}>
                {perc.watertight && perc.valid ? '✓ sound' : '✗ defect'}
              </dd>
            </div>
            <div className="flex justify-between gap-3">
              <dt className="text-muted-foreground">open edges</dt>
              <dd className={`tabular-nums ${perc.open_edges ? 'text-red-500' : 'text-muted-foreground'}`}>
                {perc.open_edges}
              </dd>
            </div>
            <div className="flex justify-between gap-3">
              <dt className="text-muted-foreground">non-manifold</dt>
              <dd className={`tabular-nums ${perc.nonmanifold_edges ? 'text-fuchsia-500' : 'text-muted-foreground'}`}>
                {perc.nonmanifold_edges}
              </dd>
            </div>
            <div className="flex justify-between gap-3">
              <dt className="text-muted-foreground">brep</dt>
              <dd className={perc.valid ? 'text-muted-foreground' : 'text-red-500'}>
                {perc.valid ? 'valid' : 'invalid'}
              </dd>
            </div>
            {perc.dims && (
              <div className="flex justify-between gap-3">
                <dt className="text-muted-foreground">bounds</dt>
                <dd className="tabular-nums text-muted-foreground">
                  {perc.dims.map((d) => Math.round(d)).join('×')}
                </dd>
              </div>
            )}
          </dl>
        )}
      </div>

      {/* Two segmented controls, and the row WRAPS. Docking took this panel from
          236px floating to the 224px dock, and nine chips on one non-wrapping
          row put `front`/`top`/`right` past the right edge with no ellipsis to
          admit it. Wrapping drops the camera group to its own line instead of
          clipping it; nothing is ever silently cut off. */}
      {/* Hug, do not distribute. Render mode and camera are PEERS — both are
          chip groups steering the same picture — so bookending them left a
          300px hole at the working width. They sit together and the row ends
          short of the right edge, which is what a group of controls should do.
          It still wraps: nine chips do not fit the 224px idle rail, and
          wrapping drops the camera group to its own line rather than clipping
          `front`/`top`/`right` with no ellipsis to admit it. */}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border px-2 py-1">
        <ModeChipGroup aria-label="Render mode">
          {modeOptions.map((m) => (
            <ModeChip
              key={m}
              selected={activeMode === m}
              onClick={() => setMode(m)}
              title={`Render mode: ${m}`}
            >
              {m === 'diagnostic' ? 'diag' : m === 'section' ? 'sec' : m}
            </ModeChip>
          ))}
        </ModeChipGroup>
        {scope === 'part' && SINGLE_VIEW.includes(mode) && (
          <ModeChipGroup aria-label="Camera">
            {(['iso', 'front', 'top', 'right'] as ViewName[]).map((v) => (
              <ModeChip
                key={v}
                selected={view === v}
                onClick={() => setView(v)}
                title={`Camera: ${v}`}
              >
                {v}
              </ModeChip>
            ))}
          </ModeChipGroup>
        )}
      </div>

      {/* Scene orbit controls: az/el step the scene-eye camera. */}
      {isSceneScope && (
        <div className="flex items-center justify-between gap-1 border-t border-border px-2 py-1 font-mono text-[11px]">
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
