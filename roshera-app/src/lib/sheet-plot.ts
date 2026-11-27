/**
 * Plotting a sheet: the order and the timing.
 *
 * A sheet opened for the first time is drawn the way a pen plotter draws it
 * (Varun, 2026-11-27: "the whole process of drawing should feel
 * mechanical"): the border and zones, the title block, each view's visible
 * edges, the hidden lines and centrelines, the dimensions and leaders, and
 * last the lettering, stamped. Linear and constant, never eased.
 *
 * This module is pure -- which layer a mark belongs to, when it goes down,
 * and whether a sheet is plotted at all. `sheet-plot-dom.ts` applies it to
 * the SVG the kernel rendered.
 */

export const PLOT_LAYERS = ['frame', 'title', 'visible', 'hidden', 'dims', 'text'] as const
export type PlotLayer = (typeof PLOT_LAYERS)[number]

/** The whole plot, whatever the size of the sheet. */
export const PLOT_TOTAL_MS = 1200

/** What the plot needs to know about one SVG element. */
export interface PlotMark {
  /** Lower-case tag name. */
  tag: string
  classes: readonly string[]
  /** Inside a `g.view`. */
  inView: boolean
  /** Inside `g.title-block`. */
  inTitleBlock: boolean
  /** Inside `g.zones`. */
  inZones: boolean
}

const has = (m: PlotMark, ...names: string[]) => names.some((n) => m.classes.includes(n))

/** The layer a mark is drawn in, or null for what is not drawn (the paper). */
export function plotLayer(m: PlotMark): PlotLayer | null {
  if (has(m, 'sheet')) return null
  if (m.tag === 'text' || m.tag === 'tspan' || m.tag === 'image') return 'text'
  if (m.inZones || has(m, 'frame')) return 'frame'
  if (m.inTitleBlock) return 'title'
  if (has(m, 'hidden', 'centerline')) return 'hidden'
  if (m.inView) return 'visible'
  return 'dims'
}

/**
 * How a mark goes down: `stroke` is drawn along its length by the pen;
 * `show` is set down whole at its moment. Lettering and pictures are
 * stamped; filled marks (arrowheads, the logo block) have no line to follow;
 * hidden lines and centrelines carry a dash pattern that IS their meaning,
 * and stroking a line works by overwriting its dash pattern.
 */
export function plotKind(m: PlotMark): 'stroke' | 'show' {
  if (plotLayer(m) === 'text') return 'show'
  if (has(m, 'hidden', 'centerline')) return 'show'
  if (m.tag === 'polygon') return 'show'
  if (m.classes.some((c) => c.includes('fill') || c.includes('arrow'))) return 'show'
  return 'stroke'
}

export interface LayerWindow {
  startMs: number
  endMs: number
}

/** Each layer's share of the plot, as a fraction of the total: the views'
 *  edges are the drawing, so they get the most pen time. */
const SHARE: Record<PlotLayer, number> = {
  frame: 0.1,
  title: 0.1,
  visible: 0.4,
  hidden: 0.12,
  dims: 0.16,
  text: 0.12,
}

/** When each layer is drawn. */
export function plotPlan(): Record<PlotLayer, LayerWindow> {
  const plan = {} as Record<PlotLayer, LayerWindow>
  let cursor = 0
  for (const layer of PLOT_LAYERS) {
    const span = SHARE[layer] * PLOT_TOTAL_MS
    plan[layer] = { startMs: cursor, endMs: cursor + span }
    cursor += span
  }
  return plan
}

/** One display frame: start times are snapped to it, so a thousand marks
 *  share a few dozen start times instead of each having its own. */
const FRAME_MS = 1000 / 60
/** The longest one stroke takes to draw. */
const STROKE_MS = 90

/** When mark `index` of `count` in a layer starts, and how long it takes. */
export function plotAt(window: LayerWindow, index: number, count: number): { atMs: number; strokeMs: number } {
  const span = window.endMs - window.startMs
  const strokeMs = Math.min(STROKE_MS, span)
  const room = span - strokeMs
  const fraction = count > 1 ? Math.min(Math.max(index, 0), count - 1) / (count - 1) : 0
  const snapped = Math.floor((fraction * room) / FRAME_MS) * FRAME_MS
  return { atMs: window.startMs + snapped, strokeMs }
}

/**
 * Whether opening this sheet plots it. Only the first time a sheet is shown
 * in a session: an edit, a re-render, a pan or a zoom shows the same sheet
 * again and must never replot it.
 */
export function shouldPlot(alreadyPlotted: ReadonlySet<string>, drawingId: string | null, reducedMotion: boolean): boolean {
  if (reducedMotion || drawingId === null) return false
  return !alreadyPlotted.has(drawingId)
}
