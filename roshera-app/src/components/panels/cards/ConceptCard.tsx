import { useContext, useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react'
import { Lightbulb } from '@/components/ui/icons'
import type { ConceptCard as ConceptCardData } from '@/lib/blackboard-cards'
import type { Concept } from '@/lib/concept-recipe'
import { boundsOfSteps, estimateVolume, massGrams, prepareRecipe, type Bounds } from '@/lib/concept-sdf'
import { loadPathSigma, loadPathStats, makerStats, type LoadPathStats, type MakerStats } from '@/lib/concept-pov'
import { POV_CODE, type PovCode } from '@/lib/concept-glsl'
import { buildRequestText, conceptNumberText, fmtEstimate, sizeEstimateText } from '@/lib/concept-build'
import {
  DEFAULT_VIEW,
  drawSketch,
  projectToCanvas,
  sketchColors,
  type OrbitView,
  type ViewFrame,
} from '@/lib/concept-renderer'
import { cn } from '@/lib/utils'
import { CardShell, Chip } from './card-chrome'
import { CardActionsContext } from './card-actions-context'
import { RevealContext } from './reveal-context'

/**
 * CONCEPT CARD
 * ============
 * The `roshera:concept` fence (concept board, spec 2026-11-27): up to three
 * design concepts the agent proposes BEFORE it builds, each a small
 * rotatable 3D sketch of its recipe. Honest by construction:
 *  - every concept wears a dashed, neutral "concept · not yet a part" chip --
 *    never a soundness tick, never a pass colour;
 *  - every number says "estimate" and how it was obtained (the board's own
 *    volume / mass by sampling the sketch's field; the agent's numbers with
 *    their stated basis);
 *  - each point of view (Load path, Maker) is a labelled heuristic;
 *  - "Build this one" never makes geometry here: it sends the request as the
 *    user's next turn and the agent builds the concept in the kernel, where
 *    it is certified like any part.
 *
 * Layout: three across when the card is at least 600 px wide; in the docked
 * Blackboard (320-520 px) the concepts stack, each a sketch beside its text.
 */

type Pov = 'sketch' | 'load' | 'maker'
type MakerMode = 'mill' | 'print'

const REVEAL_STEPS = [0.25, 0.5, 0.75, 1]
const REVEAL_STEP_MS = 70

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

interface SketchProps {
  concept: Concept
  bounds: Bounds
  pov: PovCode
  maker: MakerStats | null
  animate: boolean
}

/** One concept's 3D sketch: drag to orbit, arrow keys to turn, double-click to reset. */
function ConceptSketch({ concept, bounds, pov, maker, animate }: SketchProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const wrapRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<OrbitView>({ ...DEFAULT_VIEW })
  const dragRef = useRef<{ x: number; y: number } | null>(null)
  const frameRef = useRef<number | null>(null)
  const [reveal, setReveal] = useState(() => (animate && !prefersReducedMotion() ? 0 : 1))
  const [size, setSize] = useState<{ w: number; h: number } | null>(null)
  const [noGl, setNoGl] = useState(false)
  const sigma = useMemo(() => loadPathSigma(bounds), [bounds])

  // The sketch prints top to bottom in a few fixed steps, like the board.
  useEffect(() => {
    if (reveal >= 1) return
    const next = REVEAL_STEPS.find((r) => r > reveal) ?? 1
    const t = window.setTimeout(() => setReveal(next), REVEAL_STEP_MS)
    return () => window.clearTimeout(t)
  }, [reveal])

  useEffect(() => {
    const el = wrapRef.current
    if (!el) return
    const ro = new ResizeObserver(([entry]) => {
      const w = Math.round(entry.contentRect.width)
      const h = Math.round(entry.contentRect.height)
      setSize((s) => (s && s.w === w && s.h === h ? s : { w, h }))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const draw = useMemo(
    () => () => {
      const canvas = canvasRef.current
      if (!canvas || !size || size.w === 0 || size.h === 0) return
      const dpr = Math.min(2, window.devicePixelRatio || 1)
      const w = Math.round(size.w * dpr)
      const h = Math.round(size.h * dpr)
      if (canvas.width !== w) canvas.width = w
      if (canvas.height !== h) canvas.height = h
      const frame = drawSketch(canvas, {
        recipe: concept.recipe,
        bounds,
        view: viewRef.current,
        pov,
        reveal,
        loads: concept.loads,
        supports: concept.supports,
        sigma,
        zmin: maker?.zmin ?? bounds.min[2],
        cell: maker?.cell_mm ?? 0,
        colors: sketchColors(),
      })
      if (!frame) {
        setNoGl(true)
        return
      }
      if (pov === POV_CODE.load) drawLoadMarks(canvas, frame, concept, dpr)
    },
    [concept, bounds, pov, reveal, sigma, maker, size],
  )

  useEffect(() => {
    draw()
  }, [draw])

  const schedule = () => {
    if (frameRef.current !== null) return
    frameRef.current = window.requestAnimationFrame(() => {
      frameRef.current = null
      draw()
    })
  }
  useEffect(
    () => () => {
      if (frameRef.current !== null) window.cancelAnimationFrame(frameRef.current)
    },
    [],
  )

  const turn = (dYaw: number, dPitch: number) => {
    const v = viewRef.current
    v.yaw += dYaw
    v.pitch = Math.max(-1.45, Math.min(1.45, v.pitch + dPitch))
    schedule()
  }
  const onPointerDown = (e: PointerEvent<HTMLCanvasElement>) => {
    e.stopPropagation()
    dragRef.current = { x: e.clientX, y: e.clientY }
    // Capture keeps the drag when the pointer leaves the sketch; a pointer
    // the browser does not track (a synthetic one) cannot be captured, and
    // the drag still works while it stays over the canvas.
    try {
      e.currentTarget.setPointerCapture(e.pointerId)
    } catch {
      /* not capturable: drag without capture */
    }
  }
  const onPointerMove = (e: PointerEvent<HTMLCanvasElement>) => {
    const d = dragRef.current
    if (!d) return
    turn(-(e.clientX - d.x) * 0.012, (e.clientY - d.y) * 0.012)
    dragRef.current = { x: e.clientX, y: e.clientY }
  }
  const onPointerUp = (e: PointerEvent<HTMLCanvasElement>) => {
    dragRef.current = null
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId)
  }
  const onKeyDown = (e: KeyboardEvent<HTMLCanvasElement>) => {
    const step = Math.PI / 12
    const keys: Record<string, [number, number]> = {
      ArrowLeft: [step, 0],
      ArrowRight: [-step, 0],
      ArrowUp: [0, step],
      ArrowDown: [0, -step],
    }
    const k = keys[e.key]
    if (!k) return
    e.preventDefault()
    e.stopPropagation()
    turn(k[0], k[1])
  }

  return (
    <div ref={wrapRef} className="relative aspect-square w-full overflow-hidden rounded border border-border/50 bg-background/40">
      {noGl ? (
        <div className="flex h-full items-center justify-center p-2 text-center text-[11px] text-muted-foreground">
          No WebGL here, so the sketch cannot be drawn. The recipe and its estimates below still stand.
        </div>
      ) : (
        <canvas
          ref={canvasRef}
          className="block h-full w-full cursor-grab touch-none active:cursor-grabbing cad-focus"
          tabIndex={0}
          role="img"
          aria-label={`Concept ${concept.id} sketch, ${concept.title}. Drag or use the arrow keys to turn it.`}
          title="Drag to turn · arrow keys · double-click to reset"
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onDoubleClick={(e) => {
            e.stopPropagation()
            viewRef.current = { ...DEFAULT_VIEW }
            schedule()
          }}
          onKeyDown={onKeyDown}
        />
      )}
      {/* Over the sketch only where the sketch is wide enough to carry it;
          a stacked (docked) panel shows the same chip beside its title. */}
      <span className="pointer-events-none absolute left-1 top-1 hidden rounded border border-dashed border-muted-foreground/50 bg-background/70 px-1 font-board text-[11px] uppercase leading-4 tracking-wide text-muted-foreground @[600px]:inline">
        concept · not yet a part
      </span>
    </div>
  )
}

/** Supports as small grounded triangles, loads as arrows along their direction. */
function drawLoadMarks(canvas: HTMLCanvasElement, frame: ViewFrame, concept: Concept, dpr: number) {
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  const w = canvas.width
  const h = canvas.height
  const ink = getComputedStyle(canvas).color
  ctx.save()
  ctx.strokeStyle = ink
  ctx.fillStyle = ink
  ctx.lineWidth = 1.5 * dpr
  const s = 6 * dpr
  for (const sup of concept.supports ?? []) {
    const [x, y] = projectToCanvas(sup.at, frame, w, h)
    ctx.beginPath()
    ctx.moveTo(x, y)
    ctx.lineTo(x - s, y + 1.6 * s)
    ctx.lineTo(x + s, y + 1.6 * s)
    ctx.closePath()
    ctx.stroke()
    ctx.beginPath()
    ctx.moveTo(x - 1.4 * s, y + 2 * s)
    ctx.lineTo(x + 1.4 * s, y + 2 * s)
    ctx.stroke()
  }
  for (const load of concept.loads ?? []) {
    const [x1, y1] = projectToCanvas(load.at, frame, w, h)
    const n = Math.hypot(...load.dir)
    const len = 0.22 * frame.half * 2
    const tail: [number, number, number] = [
      load.at[0] - (load.dir[0] / n) * len,
      load.at[1] - (load.dir[1] / n) * len,
      load.at[2] - (load.dir[2] / n) * len,
    ]
    const [x0, y0] = projectToCanvas(tail, frame, w, h)
    ctx.beginPath()
    ctx.moveTo(x0, y0)
    ctx.lineTo(x1, y1)
    ctx.stroke()
    const a = Math.atan2(y1 - y0, x1 - x0)
    ctx.beginPath()
    ctx.moveTo(x1, y1)
    ctx.lineTo(x1 - s * Math.cos(a - 0.45), y1 - s * Math.sin(a - 0.45))
    ctx.lineTo(x1 - s * Math.cos(a + 0.45), y1 - s * Math.sin(a + 0.45))
    ctx.closePath()
    ctx.fill()
  }
  ctx.restore()
}

function Segment({
  active,
  disabled,
  title,
  onClick,
  children,
}: {
  active: boolean
  disabled?: boolean
  title: string
  onClick: () => void
  children: string
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      title={title}
      aria-pressed={active}
      onClick={(e) => {
        e.stopPropagation()
        onClick()
      }}
      className={cn(
        'cad-focus rounded-sm px-1.5 py-0.5 font-board text-[11px] uppercase tracking-wide',
        active ? 'cad-row-selected text-foreground' : 'cad-row-hover text-muted-foreground',
        disabled && 'cursor-not-allowed opacity-40',
      )}
    >
      {children}
    </button>
  )
}

const pct = (x: number) => `${Math.round(x * 100)}%`

function povLegend(pov: Pov, mode: MakerMode, load: LoadPathStats | null, maker: MakerStats | null): string {
  if (pov === 'sketch') return 'Sketch of the recipe · drag to turn'
  if (pov === 'load')
    return load
      ? `Load path · ${pct(load.on_path_fraction)} of the material on the straight load-to-support lines · estimate`
      : 'Load path · estimating…'
  if (!maker) return 'Maker · estimating…'
  if (mode === 'mill') {
    const setups =
      maker.setups === 1 ? 'one setup from +Z' : maker.setups === 2 ? 'two setups, +Z and −Z' : `${pct(maker.mill.neither)} not reachable from ±Z`
    return `3-axis mill · ${setups} · hatched: from −Z only · estimate`
  }
  return maker.fdm.overhang > 0.005
    ? `FDM print · ${pct(maker.fdm.overhang)} of the surface overhangs past 45° · estimate`
    : 'FDM print · no overhang past 45° on this orientation · estimate'
}

function ConceptPanel({
  concept,
  density,
  animate,
  setLabel,
}: {
  concept: Concept
  density: number | null
  animate: boolean
  setLabel: { id?: string; title?: string }
}) {
  const actions = useContext(CardActionsContext)
  const [pov, setPov] = useState<Pov>('sketch')
  const [mode, setMode] = useState<MakerMode>('mill')
  const [load, setLoad] = useState<LoadPathStats | null>(null)
  const [maker, setMaker] = useState<MakerStats | null>(null)
  const [sent, setSent] = useState(false)
  const bounds = useMemo(() => boundsOfSteps(prepareRecipe(concept.recipe)), [concept.recipe])
  const volume = useMemo(() => estimateVolume(concept.recipe).volume_mm3, [concept.recipe])
  const mass = density !== null ? massGrams(volume, density) : null
  const hasLoads = concept.loads !== undefined && concept.supports !== undefined

  // The point-of-view numbers are computed only when that view is opened,
  // after the click has painted.
  useEffect(() => {
    if (pov === 'load' && load === null && concept.loads && concept.supports) {
      const loads = concept.loads
      const supports = concept.supports
      const t = window.setTimeout(() => setLoad(loadPathStats(concept.recipe, loads, supports, 40)), 0)
      return () => window.clearTimeout(t)
    }
    if (pov === 'maker' && maker === null) {
      const t = window.setTimeout(() => setMaker(makerStats(concept.recipe, 36)), 0)
      return () => window.clearTimeout(t)
    }
  }, [pov, load, maker, concept])

  const povCode: PovCode =
    pov === 'load' ? POV_CODE.load : pov === 'maker' ? (mode === 'mill' ? POV_CODE.mill : POV_CODE.print) : POV_CODE.sketch

  return (
    <div className="flex min-w-0 flex-row gap-2.5 @[600px]:flex-col">
      <div className="w-[136px] shrink-0 @[600px]:w-full">
        <ConceptSketch concept={concept} bounds={bounds} pov={povCode} maker={maker} animate={animate} />
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 items-baseline gap-1.5">
          <span className="font-board text-[13px] text-foreground">{concept.id}</span>
          <span className="min-w-0 truncate font-medium text-foreground/90" title={concept.title}>
            {concept.title}
          </span>
        </div>
        <span className="self-start whitespace-nowrap rounded border border-dashed border-muted-foreground/50 px-1 font-board text-[11px] uppercase leading-4 tracking-wide text-muted-foreground @[600px]:hidden">
          concept · not yet a part
        </span>
        <div className="flex flex-wrap items-center gap-0.5" role="group" aria-label="Point of view">
          <Segment active={pov === 'sketch'} title="The concept as sketched" onClick={() => setPov('sketch')}>
            sketch
          </Segment>
          <Segment
            active={pov === 'load'}
            disabled={!hasLoads}
            title={hasLoads ? 'Material on the straight lines from each load to each support, the rest faded (estimate)' : 'This concept names no loads and supports'}
            onClick={() => setPov('load')}
          >
            load path
          </Segment>
          <Segment active={pov === 'maker'} title="How it is made: 3-axis milling reach from ±Z, or FDM overhangs (estimate)" onClick={() => setPov('maker')}>
            maker
          </Segment>
          {pov === 'maker' && (
            <span className="ml-1 flex items-center gap-0.5 border-l border-border/60 pl-1">
              <Segment active={mode === 'mill'} title="A straight tool along Z from above and below" onClick={() => setMode('mill')}>
                mill
              </Segment>
              <Segment active={mode === 'print'} title="FDM, printed as drawn: surfaces past 45° downward overhang" onClick={() => setMode('print')}>
                print
              </Segment>
            </span>
          )}
        </div>
        <div className="text-[11px] leading-snug text-muted-foreground">{povLegend(pov, mode, load, maker)}</div>
        <ul className="text-[11px] leading-snug text-foreground/85">
          <li className="cad-readout">
            {mass !== null ? `${fmtEstimate(mass)} g` : `${fmtEstimate(volume / 1000)} cm³`}
            <span className="text-muted-foreground">
              {' '}
              · estimate, sampled volume{mass !== null ? ` ${fmtEstimate(volume / 1000)} cm³ × ${fmtEstimate(density ?? 0)} kg/m³` : ''}
            </span>
          </li>
          {(concept.numbers ?? []).map((n, i) => (
            <li key={i}>{conceptNumberText(n)}</li>
          ))}
        </ul>
        {concept.note && <div className="text-[11px] leading-snug text-foreground/80">{concept.note}</div>}
        <div className="mt-auto pt-1">
          <button
            type="button"
            disabled={actions === null || sent}
            title={
              actions === null
                ? 'Only on the live board: the request goes to the agent, which builds it in the kernel'
                : 'Ask the agent to build this concept in the kernel; the built part is then certified'
            }
            onClick={(e) => {
              e.stopPropagation()
              if (!actions) return
              actions.requestBuild(buildRequestText(setLabel, concept))
              setSent(true)
            }}
            className={cn(
              'cad-focus rounded-md border px-2 py-0.5 text-[11px]',
              sent
                ? 'border-dashed border-border/70 text-muted-foreground'
                : 'cad-row-hover border-border/70 text-foreground/90 disabled:cursor-not-allowed disabled:opacity-50',
            )}
          >
            {sent ? 'build requested — the agent builds and certifies it' : 'Build this one'}
          </button>
        </div>
      </div>
    </div>
  )
}

export function ConceptCard({ card }: { card: ConceptCardData }) {
  const { animate } = useContext(RevealContext)
  const density = card.material?.density ?? null
  // Sampled once per payload, not on every re-render of the line.
  const summary = useMemo(
    () =>
      card.concepts.map((c) => {
        const v = estimateVolume(c.recipe).volume_mm3
        return `${c.id} ${sizeEstimateText(v, density !== null ? massGrams(v, density) : null)}${c.note ? `, ${c.note}` : ''}`
      }),
    [card.concepts, density],
  )
  return (
    <CardShell
      accent="neutral"
      icon={Lightbulb}
      title={card.title ?? 'Concepts'}
      titleText={card.title ?? 'Concepts'}
      chip={
        <Chip accent="neutral" dashed title="A proposal sketched from a recipe. Nothing is built or certified until you pick one and the kernel builds it.">
          concept · not yet a part
        </Chip>
      }
    >
      <div className="mt-2 grid gap-3 @[600px]:grid-cols-3">
        {card.concepts.map((c) => (
          <ConceptPanel key={c.id} concept={c} density={density} animate={animate} setLabel={{ id: card.id, title: card.title }} />
        ))}
      </div>
      <div className="mt-2 border-t border-border/50 pt-1.5 text-[11px] leading-snug text-foreground/85">
        <span className="cad-readout">{summary.join(' — ')}</span>
        <span className="text-muted-foreground">
          {' '}
          · estimates{card.material ? `, ${card.material.name} at ${fmtEstimate(card.material.density)} kg/m³` : ''}
        </span>
      </div>
    </CardShell>
  )
}
