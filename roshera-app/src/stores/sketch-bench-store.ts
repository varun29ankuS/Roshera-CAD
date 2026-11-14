/**
 * The sketch workbench (Varun 2026-11-14: "the sketching is severely
 * underworked ... there are no buttons that show up for sketching, for
 * editing, for trimming, for mirror ...").
 *
 * One open constraint sketch (`/api/csketch`): its plane, the active tool,
 * the clicks so far, what is selected, and what the server says the sketch
 * holds. A tool's clicks become a plan (`lib/sketch-bench.planShape`) that is
 * DRAWN AT ONCE as a ghost and sent in order; the server's sketch replaces
 * the ghost when it lands -- a click never waits on the network to show.
 *
 * The cursor lives in its own tiny store (`useBenchCursor`) so a mouse move
 * re-renders only the rubber band, never the sketch or the toolbar.
 */

import { create } from 'zustand'
import { timelineAction } from '@/lib/history'
import {
  csketchApi,
  type Constraint,
  type CSketchPlane,
  type CSketchSummary,
  type DimensionalConstraint,
  type DofReport,
  type EntityRef,
} from '@/lib/csketch-api'
import {
  extendBoundary,
  isCorner,
  nearerEnd,
  planShape,
  sameRef,
  TOOL_CLICKS,
  type ConstraintOffer,
  type DimensionOffer,
  type BenchTool,
  type Click,
  type PlanOp,
  type SnapKind,
  type Vec2,
} from '@/lib/sketch-bench'

const API_BASE = `${(import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''}/api`

export type FinishOp = 'extrude' | 'cut' | 'revolve'

/** Sizes the Modify tools use (shown and edited in the bar). */
export interface ModifyOptions {
  fillet: number
  chamfer: number
  offset: number
  linCount: number
  linDx: number
  linDy: number
  circCount: number
  /** Total angle the circular pattern spans, degrees (360 = all round). */
  circAngle: number
}

interface BenchState {
  /** The open sketch's server id; null when no sketch is open. */
  sketchId: string | null
  plane: CSketchPlane
  /** The part the sketch was started on (a face), the default Cut target. */
  onPart: string | null
  tool: BenchTool
  clicks: Click[]
  /** Sides for the polygon tool. */
  sides: number
  /** Grid step in mm (0 = off). */
  grid: number
  summary: CSketchSummary | null
  dof: DofReport | null
  selection: EntityRef[]
  /** Drawn at once while the server builds it. */
  ghost: PlanOp[]
  busy: boolean
  /** The last refusal, in the server's words. */
  error: string | null
  /** What each commit created, newest last (Ctrl+Z inside the sketch). */
  history: EntityRef[][]
  finish: { op: FinishOp; distance: number; angle: number }
  /** The sketch's constraints (glyphs and dimensions on the sketch). */
  constraints: Constraint[]
  /** A constraint picked on the sketch (Delete removes it). */
  selectedConstraint: string | null
  opts: ModifyOptions
  setOpts: (o: Partial<ModifyOptions>) => void
  /** A selection action waiting for its sizes in the bar. */
  action: 'offset' | 'linear-pattern' | null
  setAction: (a: 'offset' | 'linear-pattern' | null) => void
  /** A Modify tool's click: what it hit, and where. */
  pick: (at: Vec2, ref: EntityRef | null, pointId?: string) => Promise<void>
  applyConstraint: (offer: ConstraintOffer) => Promise<void>
  /** Pin a point where it is (its X and Y held): how a sketch stops sliding. */
  fixPoint: (pointId: string) => Promise<void>
  addDimension: (offer: DimensionOffer, value: number) => Promise<void>
  editDimension: (cid: string, value: number) => Promise<void>
  selectConstraint: (cid: string | null) => void
  /** A placed dimension being changed in the bar. */
  editing: { id: string; kind: string; value: number; unit: 'mm' | 'deg' } | null
  setEditing: (e: { id: string; kind: string; value: number; unit: 'mm' | 'deg' } | null) => void
  offsetSelection: () => Promise<void>
  linearPatternSelection: () => Promise<void>
  dragPoint: (pointId: string, to: Vec2) => Promise<void>

  open: (plane: CSketchPlane, onPart?: string | null) => Promise<void>
  close: () => void
  setTool: (tool: BenchTool) => void
  setSides: (n: number) => void
  setGrid: (g: number) => void
  setFinish: (f: Partial<BenchState['finish']>) => void
  click: (c: Click) => void
  /** Enter / double-click: end a line chain or place a spline. */
  endChain: () => void
  /** Esc: drop the clicks in progress, else the tool, else the selection. */
  escape: () => void
  select: (ref: EntityRef | null, additive: boolean) => void
  deleteSelection: () => Promise<void>
  toggleConstruction: () => Promise<void>
  undo: () => Promise<void>
  /** Turn the sketch into a feature; true when it landed. */
  complete: () => Promise<boolean>
  refresh: () => Promise<void>
}

/** The cursor, snapped -- its own store so mouse moves stay cheap. */
export const useBenchCursor = create<{
  at: (Click & { kind: SnapKind }) | null
  set: (at: (Click & { kind: SnapKind }) | null) => void
}>((set) => ({ at: null, set: (at) => set({ at }) }))

const message = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** Send a plan in order; returns what it created, and its key -> id map. */
async function runPlan(id: string, plan: PlanOp[]): Promise<{ created: EntityRef[]; ids: Map<string, string> }> {
  const ids = new Map<string, string>()
  const created: EntityRef[] = []
  const resolve = (ref: string) => {
    if (ref.startsWith('id:')) return ref.slice(3)
    const got = ids.get(ref)
    if (!got) throw new Error(`plan step ${ref} was never built`)
    return got
  }
  for (const op of plan) {
    switch (op.op) {
      case 'point': {
        const r = await csketchApi.addPoint(id, { x: op.at[0], y: op.at[1] })
        ids.set(op.key, r.id)
        created.push({ Point: r.id })
        break
      }
      case 'line': {
        const r = await csketchApi.addLine(id, { start: resolve(op.a), end: resolve(op.b) })
        ids.set(op.key, r.id)
        created.push({ Line: r.id })
        if (op.construction) await csketchApi.setConstruction(id, { Line: r.id }, true)
        break
      }
      case 'circle': {
        const r = await csketchApi.addCircle(id, { cx: op.c[0], cy: op.c[1], radius: op.r })
        ids.set(op.key, r.id)
        created.push({ Circle: r.id })
        break
      }
      case 'arc': {
        // On its end points, so it joins whatever shares them.
        const r = await csketchApi.addArc(id, {
          start: resolve(op.a),
          end: resolve(op.b),
          radius: op.r,
          ccw: op.ccw,
          large_arc: op.large,
        })
        ids.set(op.key, r.id)
        created.push({ Arc: r.id })
        break
      }
      case 'spline': {
        const r = await csketchApi.addSpline(id, { degree: op.degree, control_point_ids: op.points.map(resolve) })
        ids.set(op.key, r.id)
        created.push({ Spline: r.id })
        break
      }
      case 'constrain': {
        await csketchApi.addConstraint(id, {
          id: crypto.randomUUID(),
          constraint_type: { Geometric: op.kind },
          entities: op.refs.map((r) => ({ [r.kind]: resolve(r.ref) }) as EntityRef),
          priority: 'Required',
          status: 'Satisfied',
          name: null,
        })
        break
      }
    }
  }
  return { created, ids }
}

export const useSketchBench = create<BenchState>((set, get) => {
  /** Re-read the sketch and its freedom count. */
  const refresh = async () => {
    const id = get().sketchId
    if (!id) return
    const [summary, dof, constraints] = await Promise.all([
      csketchApi.get(id),
      csketchApi.dof(id).catch(() => null),
      csketchApi.listConstraints(id).catch(() => [] as Constraint[]),
    ])
    if (get().sketchId === id) set({ summary, dof, constraints, ghost: [] })
  }

  /** Build a plan: drawn at once, then sent; the server's sketch replaces it. */
  const commit = async (plan: PlanOp[]): Promise<Map<string, string> | null> => {
    const id = get().sketchId
    if (!id || plan.length === 0) return null
    set((s) => ({ ghost: [...s.ghost, ...plan], busy: true, error: null }))
    try {
      const { created, ids } = await runPlan(id, plan)
      if (plan.some((o) => o.op === 'constrain')) await csketchApi.solve(id).catch(() => null)
      set((s) => ({ history: [...s.history, created] }))
      return ids
    } catch (e) {
      set({ error: message(e) })
      return null
    } finally {
      set({ busy: false })
      await refresh().catch((e) => set({ error: message(e) }))
    }
  }

  /** Run one sketch op: busy, refusal in the server's words, then re-read. */
  const op = async (fn: (id: string) => Promise<unknown>) => {
    const id = get().sketchId
    if (!id) return
    set({ busy: true, error: null })
    try {
      await fn(id)
    } catch (e) {
      set({ error: message(e) })
    } finally {
      set({ busy: false })
      await refresh().catch((e) => set({ error: message(e) }))
    }
  }

  /** A dimension as the solver takes it (a line's length is the distance
   *  between its two end points; angles go in radians). */
  const dimensional = (offer: DimensionOffer, value: number): { d: DimensionalConstraint; entities: EntityRef[] } => {
    const s = get().summary
    if (offer.kind === 'Length' && s) {
      const lid = (offer.entities[0] as { Line: string }).Line
      const ends = s.lines.find((l) => l.id === lid)?.endpoints
      if (ends) return { d: { Distance: value }, entities: [{ Point: ends[0] }, { Point: ends[1] }] }
      return { d: { Length: value }, entities: offer.entities }
    }
    if (offer.kind === 'Angle') return { d: { Angle: (value * Math.PI) / 180 }, entities: offer.entities }
    if (offer.kind === 'Diameter') return { d: { Diameter: value }, entities: offer.entities }
    if (offer.kind === 'Radius') return { d: { Radius: value }, entities: offer.entities }
    return { d: { Distance: value }, entities: offer.entities }
  }

  return {
    constraints: [],
    selectedConstraint: null,
    opts: { fillet: 2, chamfer: 1, offset: 2, linCount: 3, linDx: 10, linDy: 0, circCount: 6, circAngle: 360 },
    setOpts: (o) => set((s) => ({ opts: { ...s.opts, ...o } })),
    action: null,
    setAction: (action) => set({ action, error: null }),

    pick: async (at, ref, pointId) => {
      const { tool, summary, selection, opts } = get()
      if (!summary) return
      const lineId = ref && 'Line' in ref ? ref.Line : null
      switch (tool) {
        case 'trim':
          if (!ref || 'Point' in ref) return set({ error: 'click the piece of a line, arc or circle to cut away' })
          return op((id) => csketchApi.trimAt(id, ref, at))
        case 'extend': {
          if (!lineId) return set({ error: 'click near the end of a line to extend it' })
          const end = nearerEnd(lineId, at, summary)
          const boundary = end && extendBoundary(lineId, end, summary)
          if (!end || !boundary) return set({ error: 'nothing lies ahead of that end to extend to' })
          return op((id) => csketchApi.extend(id, { entity: { Line: lineId }, end, boundary }))
        }
        case 'fillet':
        case 'chamfer': {
          // A corner point, or the line end nearest the click.
          let corner = ref && 'Point' in ref ? ref.Point : null
          if (!corner && lineId) {
            const l = summary.lines.find((x) => x.id === lineId)
            const end = nearerEnd(lineId, at, summary)
            if (l?.endpoints && end) corner = end === 'start' ? l.endpoints[0] : l.endpoints[1]
          }
          if (!corner || !isCorner(corner, summary)) {
            return set({ error: 'click a corner -- a point where exactly two lines meet' })
          }
          const size = tool === 'fillet' ? opts.fillet : opts.chamfer
          return op((id) => (tool === 'fillet' ? csketchApi.fillet(id, corner, size) : csketchApi.chamfer(id, corner, size)))
        }
        case 'mirror': {
          if (!lineId) return set({ error: 'click the line to mirror about' })
          const what = selection.filter((r) => !sameRef(r, { Line: lineId }))
          if (what.length === 0) return set({ error: 'select what to mirror first, then click the mirror line' })
          return op(async (id) => {
            // The mirror line becomes a centre line, as in every CAD sketcher.
            const axis = summary.lines.find((l) => l.id === lineId)
            if (axis && !axis.is_construction) await csketchApi.setConstruction(id, { Line: lineId }, true)
            await csketchApi.mirror(id, { entities: what, axis: lineId })
            set({ selection: [] })
          })
        }
        case 'circular-pattern': {
          if (selection.length === 0) return set({ error: 'select what to pattern first, then click the centre' })
          const n = Math.max(2, Math.round(opts.circCount))
          const full = Math.abs(opts.circAngle - 360) < 1e-9
          const step = ((full ? 360 / n : opts.circAngle / (n - 1)) * Math.PI) / 180
          return op(async (id) => {
            await csketchApi.circularPattern(id, {
              entities: selection,
              ...(pointId ? { center: pointId } : { center_position: at }),
              count: n,
              angle_step: step,
            })
            set({ selection: [] })
          })
        }
      }
    },

    applyConstraint: (offer) =>
      op(async (id) => {
        await csketchApi.addConstraint(id, {
          id: crypto.randomUUID(),
          constraint_type: { Geometric: offer.kind },
          entities: offer.entities,
          priority: 'Required',
          status: 'Satisfied',
          name: null,
        })
        await csketchApi.solve(id)
      }),

    fixPoint: (pointId) =>
      op(async (id) => {
        const p = get().summary?.points.find((x) => x.id === pointId)
        if (!p) throw new Error('that point is not in the sketch')
        for (const d of [{ XCoordinate: p.x }, { YCoordinate: p.y }] as DimensionalConstraint[]) {
          await csketchApi.addConstraint(id, {
            id: crypto.randomUUID(),
            constraint_type: { Dimensional: d },
            entities: [{ Point: pointId }],
            priority: 'Required',
            status: 'Satisfied',
            name: null,
          })
        }
        await csketchApi.solve(id)
        set({ selection: [] })
      }),

    addDimension: (offer, value) =>
      op(async (id) => {
        const { d, entities } = dimensional(offer, value)
        const cid = crypto.randomUUID()
        await csketchApi.addConstraint(id, {
          id: cid,
          constraint_type: { Dimensional: d },
          entities,
          priority: 'Required',
          status: 'Satisfied',
          name: null,
        })
        const report = await csketchApi.solve(id)
        // A dimension the sketch cannot meet is taken back, with the reason.
        const st = (report as { status?: { kind?: string } }).status?.kind
        if (st === 'over_constrained' || st === 'not_converged' || st === 'unstable') {
          await csketchApi.deleteConstraint(id, cid)
          await csketchApi.solve(id).catch(() => null)
          throw new Error('that dimension fights the sketch (over-defined): it was not added')
        }
        set({ selection: [] })
      }),

    editDimension: (cid, value) => op((id) => csketchApi.updateConstraintValue(id, cid, value)),

    selectConstraint: (cid) => set({ selectedConstraint: cid }),
    editing: null,
    setEditing: (editing) => set({ editing, error: null }),

    offsetSelection: () =>
      op(async (id) => {
        const { selection, opts } = get()
        for (const r of selection) await csketchApi.offset(id, { entity: r, distance: opts.offset })
        set({ selection: [] })
      }),

    linearPatternSelection: () =>
      op(async (id) => {
        const { selection, opts } = get()
        await csketchApi.linearPattern(id, {
          entities: selection,
          count: Math.max(2, Math.round(opts.linCount)),
          dx: opts.linDx,
          dy: opts.linDy,
        })
        set({ selection: [] })
      }),

    dragPoint: (pointId, to) =>
      op((id) => csketchApi.drag(id, { entity: { Point: pointId }, target: { kind: 'point', params: { x: to[0], y: to[1] } } })),

    sketchId: null,
    plane: 'xy',
    onPart: null,
    tool: 'line',
    clicks: [],
    sides: 6,
    grid: 1,
    summary: null,
    dof: null,
    selection: [],
    ghost: [],
    busy: false,
    error: null,
    history: [],
    finish: { op: 'extrude', distance: 10, angle: 360 },

    open: async (plane, onPart = null) => {
      set({ busy: true, error: null })
      try {
        const { id } = await csketchApi.create()
        set({
          sketchId: id,
          plane,
          onPart,
          tool: 'line',
          clicks: [],
          selection: [],
          ghost: [],
          history: [],
          summary: null,
          dof: null,
          finish: { op: onPart ? 'cut' : 'extrude', distance: 10, angle: 360 },
        })
        await refresh()
      } catch (e) {
        set({ error: message(e) })
      } finally {
        set({ busy: false })
      }
    },

    close: () => {
      useBenchCursor.getState().set(null)
      set({
        sketchId: null,
        summary: null,
        dof: null,
        clicks: [],
        selection: [],
        ghost: [],
        history: [],
        error: null,
        constraints: [],
        selectedConstraint: null,
      })
    },

    // Mirror and circular pattern act on the selection: keep it.
    setTool: (tool) =>
      set((s) => ({
        tool,
        clicks: [],
        error: null,
        selection: tool === 'mirror' || tool === 'circular-pattern' || tool === 'dimension' || tool === 'select' ? s.selection : [],
      })),
    setSides: (n) => set({ sides: Math.max(3, Math.min(64, Math.round(n))) }),
    setGrid: (g) => set({ grid: Math.max(0, g) }),
    setFinish: (f) => set((s) => ({ finish: { ...s.finish, ...f } })),

    click: (c) => {
      const { tool, clicks, sides } = get()
      if (tool === 'select') return
      const next = [...clicks, c]
      const need = TOOL_CLICKS[tool]
      if (tool === 'line' && next.length === 2) {
        // A line chain: each click ends one line and starts the next ON
        // the point it just made, so the chain stays joined.
        const plan = planShape('line', next)
        const endKey = [...plan].reverse().find((o) => o.op === 'point')?.key
        const end = plan.find((o) => o.op === 'point' && o.key === endKey)
        const endAt: Vec2 = end && end.op === 'point' ? end.at : c.at
        set({ clicks: [{ at: endAt, pointId: c.pointId }] })
        void commit(plan).then((ids) => {
          // Continue from the real point id once it exists.
          if (ids && endKey && !c.pointId) {
            const pid = ids.get(endKey)
            const cur = get().clicks
            if (pid && cur.length === 1 && cur[0].at === endAt) set({ clicks: [{ at: endAt, pointId: pid }] })
          }
        })
        return
      }
      if (need === 'chain' || next.length < need) {
        set({ clicks: next })
        return
      }
      set({ clicks: [] })
      void commit(planShape(tool, next, { sides }))
    },

    endChain: () => {
      const { tool, clicks } = get()
      if (tool === 'spline' && clicks.length >= 2) void commit(planShape('spline', clicks))
      set({ clicks: [] })
    },

    escape: () => {
      const { clicks, tool, selection } = get()
      if (clicks.length > 0) set({ clicks: [] })
      else if (tool !== 'select') set({ tool: 'select' })
      else if (selection.length > 0) set({ selection: [] })
    },

    select: (ref, additive) => {
      if (!ref) {
        if (!additive) set({ selection: [] })
        return
      }
      const sel = get().selection
      const has = sel.some((r) => sameRef(r, ref))
      if (additive) set({ selection: has ? sel.filter((r) => !sameRef(r, ref)) : [...sel, ref] })
      else set({ selection: [ref] })
    },

    deleteSelection: async () => {
      const { sketchId, selection, selectedConstraint } = get()
      if (sketchId && selectedConstraint) {
        set({ selectedConstraint: null })
        return op((id) => csketchApi.deleteConstraint(id, selectedConstraint))
      }
      if (!sketchId || selection.length === 0) return
      set({ busy: true, error: null })
      try {
        const summary = await csketchApi.deleteEntities(sketchId, selection)
        set({ summary, selection: [] })
        await refresh()
      } catch (e) {
        set({ error: message(e) })
      } finally {
        set({ busy: false })
      }
    },

    toggleConstruction: async () => {
      const { sketchId, selection, summary } = get()
      if (!sketchId || !summary || selection.length === 0) return
      const isCon = (r: EntityRef): boolean => {
        const [k, id] = Object.entries(r)[0]
        const list = (
          {
            Point: summary.points,
            Line: summary.lines,
            Arc: summary.arcs,
            Circle: summary.circles,
            Rectangle: summary.rectangles,
            Ellipse: summary.ellipses,
            Spline: summary.splines,
            Polyline: summary.polylines,
          } as Record<string, { id: string; is_construction: boolean }[]>
        )[k]
        return list?.find((e) => e.id === id)?.is_construction ?? false
      }
      // All become construction unless all already are.
      const to = !selection.every(isCon)
      set({ busy: true, error: null })
      try {
        for (const r of selection) await csketchApi.setConstruction(sketchId, r, to)
        await refresh()
      } catch (e) {
        set({ error: message(e) })
      } finally {
        set({ busy: false })
      }
    },

    undo: async () => {
      const { sketchId, history, clicks } = get()
      if (clicks.length > 0) {
        set({ clicks: [] })
        return
      }
      const last = history[history.length - 1]
      if (!sketchId || !last) return
      set({ history: history.slice(0, -1), busy: true, error: null })
      try {
        // Curves first; their own points go with them unless shared.
        const curves = last.filter((r) => !('Point' in r))
        const points = last.filter((r) => 'Point' in r)
        const present = (r: EntityRef) => {
          const s = get().summary
          if (!s) return false
          const [k, id] = Object.entries(r)[0]
          if (k === 'Point') return s.points.some((p) => p.id === id)
          return true
        }
        const targets = curves.length > 0 ? curves : points.filter(present)
        if (targets.length > 0) await csketchApi.deleteEntities(sketchId, targets)
        await refresh()
      } catch (e) {
        set({ error: message(e) })
      } finally {
        set({ busy: false })
      }
    },

    complete: async () => {
      const { sketchId, plane, finish, selection, summary, onPart } = get()
      if (!sketchId) return false
      set({ busy: true, error: null })
      try {
        if (finish.op === 'revolve') {
          // The axis is a selected line (usually a centre line).
          const axisRef = selection.find((r) => 'Line' in r)
          const axis = axisRef && summary?.lines.find((l) => 'Line' in axisRef && l.id === axisRef.Line)
          if (!axis || !('Segment' in axis.geometry)) {
            throw new Error('select the line to revolve about first (a centre line works best)')
          }
          const { start, end } = axis.geometry.Segment
          await csketchApi.revolve(sketchId, {
            axis_origin: [start.x, start.y],
            axis_direction: [end.x - start.x, end.y - start.y],
            angle: (finish.angle * Math.PI) / 180,
            plane,
          })
        } else {
          const made = await csketchApi.extrude(sketchId, {
            distance: finish.op === 'cut' ? -Math.abs(finish.distance) : finish.distance,
            plane,
          })
          if (finish.op === 'cut') {
            const cutter = made.object?.id
            const target = onPart
            if (!cutter || !target) throw new Error('nothing to cut: start the sketch on a face of the part')
            const r = await fetch(`${API_BASE}/geometry/boolean`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ operation: 'difference', object_a: target, object_b: cutter }),
            })
            if (!r.ok) {
              const body = (await r.json().catch(() => ({}))) as { error?: string; message?: string }
              // Take the tool body back out: a failed cut leaves no stray
              // solid and no step in the history.
              await timelineAction('undo').catch(() => false)
              throw new Error(body.message ?? body.error ?? `cut refused (HTTP ${r.status})`)
            }
          }
        }
        get().close()
        return true
      } catch (e) {
        set({ error: message(e) })
        return false
      } finally {
        set({ busy: false })
      }
    },

    refresh,
  }
})
