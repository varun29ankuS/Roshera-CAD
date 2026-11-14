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
  type CSketchPlane,
  type CSketchSummary,
  type DofReport,
  type EntityRef,
} from '@/lib/csketch-api'
import {
  planShape,
  sameRef,
  TOOL_CLICKS,
  type BenchTool,
  type Click,
  type PlanOp,
  type SnapKind,
  type Vec2,
} from '@/lib/sketch-bench'

const API_BASE = `${(import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''}/api`

export type FinishOp = 'extrude' | 'cut' | 'revolve'

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
        const r = await csketchApi.addArc(id, {
          cx: op.c[0],
          cy: op.c[1],
          radius: op.r,
          start_angle: op.start,
          end_angle: op.end,
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
    const [summary, dof] = await Promise.all([csketchApi.get(id), csketchApi.dof(id).catch(() => null)])
    if (get().sketchId === id) set({ summary, dof, ghost: [] })
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

  return {
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
      set({ sketchId: null, summary: null, dof: null, clicks: [], selection: [], ghost: [], history: [], error: null })
    },

    setTool: (tool) => set({ tool, clicks: [], error: null }),
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
      const { sketchId, selection } = get()
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
