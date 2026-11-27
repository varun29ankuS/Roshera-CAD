import { useEffect, useMemo, useState } from 'react'
import { Check, CircleHelp, Play, Plus, Trash2, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import { useSimStore } from '@/stores/sim-store'
import { verifyPart } from '@/lib/dfm'
import { faceNormal } from '@/lib/face-normal'
import {
  STEPS,
  SUPPORT_KINDS,
  deformScale,
  engineLabel,
  finalRun,
  entryOf,
  fetchMaterials,
  fetchSession,
  fieldRange,
  flipped,
  forceOf,
  makeMesh,
  meshControlOf,
  DENSITY,
  estimateNodes,
  runApproved,
  setupGaps,
  stressColor,
  studyOf,
  submitSetup,
  verdictView,
  type LibraryMaterial,
  type LoadDirection,
  type SimState,
  type Spread,
  type StepId,
} from '@/lib/sim'
import { LOAD_COLOR, SUPPORT_COLOR } from '@/components/viewport/SimView'

const STATE_STYLE: Record<SimState, { mark: string; text: string; wash: string }> = {
  fix: { mark: 'text-red-600 dark:text-red-400', text: 'text-red-700 dark:text-red-300', wash: 'bg-red-500/10' },
  unknown: { mark: 'text-amber-600 dark:text-amber-400', text: 'text-amber-700 dark:text-amber-300', wash: 'bg-amber-500/12' },
  pass: { mark: 'text-emerald-600 dark:text-emerald-400', text: 'text-emerald-700 dark:text-emerald-300', wash: 'bg-emerald-500/10' },
}

function Mark({ state }: { state: SimState }) {
  const Icon = state === 'fix' ? X : state === 'unknown' ? CircleHelp : Check
  return <Icon size={13} className={cn('shrink-0', STATE_STYLE[state].mark)} aria-hidden />
}

const solidOf = (o: CADObject) => o.analyticalGeometry?.solidId

const field = 'cad-focus h-7 rounded border border-border/70 bg-background/70 px-1.5'

type Phase =
  | { kind: 'idle' }
  | { kind: 'busy'; what: string }
  | { kind: 'unverified'; reason: string }
  | { kind: 'can_move'; motions: string[] }
  | { kind: 'engine_missing'; reason: string }
  | { kind: 'refused'; reason: string }

const btn = 'cad-focus flex h-7 items-center justify-center gap-1.5 rounded border border-border bg-background px-2.5 font-medium disabled:opacity-50'
const primary = 'cad-focus flex h-7 items-center justify-center gap-1.5 rounded bg-foreground px-3 font-medium text-background disabled:opacity-50'

const DIRECTION_BUTTONS: { id: LoadDirection; label: string; title: string }[] = [
  { id: 'into', label: 'Push', title: 'Pushes into the faces' },
  { id: 'out', label: 'Pull', title: 'Pulls out of the faces' },
  { id: '+x', label: '+X', title: 'Along +X' },
  { id: '-x', label: '−X', title: 'Along −X' },
  { id: '+y', label: '+Y', title: 'Along +Y' },
  { id: '-y', label: '−Y', title: 'Along −Y' },
  { id: '+z', label: '+Z', title: 'Along +Z (up)' },
  { id: '-z', label: '−Z', title: 'Along −Z (down)' },
]

/**
 * The Simulate workspace's panel: a strength study as one bar of steps --
 * part, mesh, supports, loads, material, run -- with only the current step
 * open and the finished ones folded to a line each. The same bar moves when
 * an agent walks the person through the study (its questions are in the
 * Blackboard). Faces are picked live: choose a support or a force, then
 * click faces on the part; each joins at once (click again to drop it).
 * Every stage goes through the kernel's own gates (mesh -> set-up -> run),
 * so a run is always of what is on screen.
 */
export function SimPanel() {
  const objects = useSceneStore((s) => s.objects)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const picks = useSceneStore((s) => s.subElementSelections)
  const clearPicks = useSceneStore((s) => s.clearSubElementSelections)
  const addPick = useSceneStore((s) => s.addSubElementSelection)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)

  const sim = useSimStore()
  const { setup, report, step, picking } = sim

  // Faces are what a study is set up on: face picking while this is open.
  useEffect(() => {
    const was = useSceneStore.getState().selectionMode
    setSelectionMode('face')
    return () => setSelectionMode(was)
  }, [setSelectionMode])

  const parts = useMemo(
    () => [...objects.values()].filter((o) => typeof solidOf(o) === 'number' && o.visible !== false),
    [objects],
  )
  // The part you selected, else the newest: the one you are working on,
  // not the oldest in the document.
  const newest = parts.reduce<CADObject | undefined>(
    (best, p) => ((solidOf(p) ?? -1) > (best ? (solidOf(best) ?? -1) : -1) ? p : best),
    undefined,
  )
  const fallback = (parts.find((p) => selectedIds.has(p.id)) ?? newest)?.id ?? null
  const partId = sim.objectId && objects.has(sim.objectId) ? sim.objectId : fallback
  const part = partId ? objects.get(partId) : undefined
  const solidId = part ? solidOf(part) : undefined
  const { setPart } = sim
  useEffect(() => {
    if (partId !== sim.objectId) setPart(partId)
  }, [partId, sim.objectId, setPart])

  // The document's study, whoever drives it, shown at its step as it moves.
  const { loadSession, markSeen } = sim
  useEffect(() => {
    let stop = false
    const tick = async () => {
      const session = await fetchSession().catch(() => null)
      if (stop || !session || session.stale) return
      if (useSimStore.getState().revision === session.revision) return
      const owner = parts.find((p) => solidOf(p) === session.solid)
      if (owner) loadSession(owner.id, session)
    }
    void tick()
    // Not from a hidden tab: nobody is watching the study move.
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') void tick()
    }, 2000)
    return () => {
      stop = true
      clearInterval(timer)
    }
  }, [parts, loadSession])

  // Live picking: the faces selected on this part ARE the faces of the
  // support or load being picked for.
  const picked = useMemo(
    () => [...new Set(picks.filter((p) => p.type === 'face' && p.objectId === partId).map((p) => p.index))],
    [picks, partId],
  )
  const { updateSupport, updateLoad } = sim
  useEffect(() => {
    if (!picking || !partId) return
    if (picking.kind === 'support') {
      updateSupport(picking.index, { faces: picked })
    } else if (picking.kind === 'local') {
      useSimStore.getState().updateLocal(picking.index, { faces: picked })
    } else {
      const mesh = meshRefs.get(partId)
      const ids = objects.get(partId)?.mesh.faceIds
      const normal = mesh && ids && picked.length > 0 ? faceNormal(mesh, ids, new Set(picked)) : null
      updateLoad(picking.index, { faces: picked, normal })
    }
    // `picking` itself is not a dependency: starting to pick re-selects the
    // entry's own faces first (below), which then flows through here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [picked])

  const startPicking = (kind: 'support' | 'load' | 'local', index: number, faces: number[]) => {
    clearPicks()
    if (partId) for (const f of faces) addPick({ objectId: partId, type: 'face', index: f, resolved: true })
    sim.setPicking({ kind, index })
  }
  const stopPicking = () => {
    sim.setPicking(null)
    clearPicks()
  }
  // Esc ends picking.
  useEffect(() => {
    if (!picking) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') stopPicking()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [picking])

  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  const [library, setLibrary] = useState<LibraryMaterial[]>([])
  useEffect(() => {
    void fetchMaterials().then(setLibrary).catch(() => setLibrary([]))
  }, [])

  const gaps = setupGaps(setup)
  const study = studyOf(setup)
  const meshed = sim.mesh !== null
  // The part's largest extent, for the mesh density slider (mm).
  const partExtent = useMemo(() => {
    const g = partId ? meshRefs.get(partId)?.geometry : undefined
    if (!g) return null
    if (!g.boundingBox) g.computeBoundingBox()
    const b = g.boundingBox
    return b ? Math.max(b.max.x - b.min.x, b.max.y - b.min.y, b.max.z - b.min.z) : null
  }, [partId, meshRefs])

  const fail = (out: { kind: string; reason?: string; motions?: string[] }) => {
    if (out.kind === 'can_move') setPhase({ kind: 'can_move', motions: out.motions ?? [] })
    else if (out.kind === 'unverified') setPhase({ kind: 'unverified', reason: out.reason ?? '' })
    else if (out.kind === 'engine_missing') setPhase({ kind: 'engine_missing', reason: out.reason ?? '' })
    else setPhase({ kind: 'refused', reason: out.reason ?? 'refused' })
  }

  const mesh = async (verifyFirst = false) => {
    if (typeof solidId !== 'number' || !partId) return
    setPhase({ kind: 'busy', what: 'Making the mesh…' })
    if (verifyFirst && !(await verifyPart(solidId))) {
      setPhase({ kind: 'refused', reason: 'The part could not be verified.' })
      return
    }
    const settings = meshControlOf(setup)
    if ('error' in settings) {
      setPhase({ kind: 'refused', reason: settings.error })
      return
    }
    const out = await makeMesh(solidId, settings.control).catch((e) => ({
      kind: 'refused' as const,
      reason: e instanceof Error ? e.message : String(e),
    }))
    if (out.kind !== 'ok') return fail(out)
    // Keep the person's set-up; show the mesh.
    const kept = useSimStore.getState().setup
    loadSession(partId, out.session)
    useSimStore.setState({ setup: kept, step: 'mesh' })
    setPhase({ kind: 'idle' })
  }

  const run = async () => {
    if (typeof solidId !== 'number' || !partId || !study || !sim.revision) return
    setPhase({ kind: 'busy', what: 'Checking the set-up…' })
    const staged = await submitSetup(solidId, sim.revision, study).catch((e) => ({
      kind: 'refused' as const,
      reason: e instanceof Error ? e.message : String(e),
    }))
    if (staged.kind !== 'ok') return fail(staged)
    setPhase({ kind: 'busy', what: 'Solving on both meshes…' })
    const out = await runApproved(solidId, staged.session.revision).catch((e) => ({
      kind: 'refused' as const,
      reason: e instanceof Error ? e.message : String(e),
    }))
    if (out.kind !== 'report') return fail(out)
    sim.setReport(out.report)
    const session = await fetchSession().catch(() => null)
    if (session) markSeen(session.revision)
    setPhase({ kind: 'idle' })
  }

  // ── What each step needs, and its one-line summary once done ──────────
  const supportsDone = setup.supports.some((s) => s.faces.length > 0)
  const loadsDone = setup.loads.length > 0 && setup.loads.every((l) => l.faces.length > 0 && forceOf(l) !== null)
  const m = setup.material
  const materialDone = m.name.trim() !== '' && Number(m.youngs_modulus_mpa) > 0 && Number(m.poisson_ratio) > 0
  const done: Record<StepId, boolean> = {
    part: !!part,
    mesh: meshed,
    supports: supportsDone,
    loads: loadsDone,
    material: materialDone,
    run: !!report,
  }
  const summary: Record<StepId, string> = {
    part: part?.name ?? 'No part',
    mesh: sim.mesh ? `${sim.mesh.fine.nodes.toLocaleString()} nodes` : 'Not made yet',
    supports: setup.supports
      .filter((s) => s.faces.length > 0)
      .map((s) => `${SUPPORT_KINDS.find((k) => k.id === s.kind)?.label} · ${s.faces.length} face${s.faces.length === 1 ? '' : 's'}`)
      .join(', ') || 'None yet',
    loads: setup.loads.map((l) => `${Math.round(l.newtons)} N · ${l.faces.length} face${l.faces.length === 1 ? '' : 's'}`).join(', ') || 'None yet',
    material: m.name || 'Not chosen',
    run: report ? verdictView(report.verdict).title : 'Not run',
  }
  const agentDriving = sim.stage === 'mesh' || sim.stage === 'setup'
  const index = STEPS.findIndex((s) => s.id === step)
  const next = () => {
    const n = STEPS[index + 1]
    if (n) sim.setStep(n.id)
  }
  const canNext: Record<StepId, boolean> = {
    part: !!part,
    mesh: meshed,
    supports: supportsDone,
    loads: loadsDone,
    material: materialDone,
    run: false,
  }

  return (
    <section aria-label="Simulate" className="flex h-full min-h-0 flex-col text-xs">
      <div className="border-b border-border/70 px-3 py-2">
        <div className="text-[13px] font-semibold">Static strength</div>
        <div className="text-[11px] text-muted-foreground">Will the part hold its loads?</div>
      </div>

      {/* The one step bar: where the study is, for the person and the agent alike. */}
      <ol className="flex items-center gap-0.5 border-b border-border/70 px-2 py-2" aria-label="Steps">
        {STEPS.map((s, i) => {
          const here = s.id === step
          return (
            <li key={s.id} className="flex min-w-0 flex-1 items-center">
              <button
                type="button"
                onClick={() => sim.setStep(s.id)}
                aria-current={here ? 'step' : undefined}
                title={s.does}
                className={cn(
                  'cad-focus flex min-w-0 flex-1 flex-col items-center gap-0.5 rounded px-0.5 py-1',
                  here ? 'bg-sky-500/15 text-sky-900 dark:text-sky-100' : 'hover:bg-accent/60',
                )}
              >
                <span
                  className={cn(
                    'flex h-5 w-5 items-center justify-center rounded-full text-[11px] font-semibold',
                    done[s.id] ? 'bg-emerald-600 text-white' : here ? 'bg-sky-600 text-white' : 'border border-border text-muted-foreground',
                  )}
                >
                  {done[s.id] ? <Check size={11} aria-hidden /> : i + 1}
                </span>
                <span className="truncate text-[11px] font-medium">{s.label}</span>
              </button>
            </li>
          )
        })}
      </ol>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* Finished steps before this one, one line each. */}
        {STEPS.slice(0, index).map((s) => (
          <button
            key={s.id}
            type="button"
            onClick={() => sim.setStep(s.id)}
            className="cad-focus flex w-full items-center gap-2 border-b border-border/50 px-3 py-1.5 text-left hover:bg-accent/50"
          >
            <Check size={12} className={done[s.id] ? 'text-emerald-600' : 'text-muted-foreground/40'} aria-hidden />
            <span className="w-16 shrink-0 font-medium">{s.label}</span>
            <span className="truncate text-muted-foreground">{summary[s.id]}</span>
          </button>
        ))}

        {/* The current step. */}
        <div className="flex flex-col gap-2 px-3 py-3">
          <div className="flex items-baseline gap-2">
            <span className="text-[13px] font-semibold">
              {index + 1}. {STEPS[index]?.label}
            </span>
          </div>
          <p className="text-[11px] text-muted-foreground">{STEPS[index]?.does}</p>
          {agentDriving && (step === 'mesh' || step === 'loads') && (
            <p className="rounded bg-sky-500/10 px-2 py-1.5 text-[11px] text-sky-900 dark:text-sky-100">
              The agent is showing you this on the part and asking in the Blackboard whether it is right.
            </p>
          )}

          {step === 'part' && (
            <select name="part" className={cn(field, 'min-w-0')} value={partId ?? ''} onChange={(e) => sim.setPart(e.target.value)} disabled={parts.length === 0} aria-label="Part">
              {parts.length === 0 && <option value="">No parts</option>}
              {parts.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          )}

          {step === 'mesh' && (
            <MeshStep
              extent={partExtent}
              busy={phase.kind === 'busy'}
              canMesh={!!part}
              onMesh={() => void mesh()}
              picking={picking}
              onPick={(index, faces) => startPicking('local', index, faces)}
              onStopPicking={stopPicking}
            />
          )}

          {step === 'supports' && (
            <>
              {setup.supports.map((s, i) => {
                const active = picking?.kind === 'support' && picking.index === i
                return (
                  <div
                    key={i}
                    className={cn('flex flex-col gap-1 rounded border px-2 py-1.5', active ? 'border-sky-500 bg-sky-500/10' : 'border-border/60')}
                    onMouseEnter={() => sim.setFocus({ kind: 'support', index: i })}
                    onMouseLeave={() => sim.setFocus(null)}
                  >
                    <div className="flex items-center gap-1.5">
                      <span className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ background: SUPPORT_COLOR }} aria-hidden />
                      <span className="font-medium">{SUPPORT_KINDS.find((k) => k.id === s.kind)?.label}</span>
                      <span className="flex-1 text-muted-foreground">
                        {s.faces.length} face{s.faces.length === 1 ? '' : 's'}
                      </span>
                      <button type="button" className={btn} onClick={() => (active ? stopPicking() : startPicking('support', i, s.faces))}>
                        {active ? 'Done' : 'Pick faces'}
                      </button>
                      <button type="button" className="cad-focus p-1 text-muted-foreground hover:text-foreground" onClick={() => { if (active) stopPicking(); sim.removeSupport(i) }} aria-label="Remove support">
                        <Trash2 size={12} />
                      </button>
                    </div>
                    {active && <span className="text-[11px] text-sky-800 dark:text-sky-200">Click faces on the part — each turns blue. Click again to remove. Esc or Done to finish.</span>}
                  </div>
                )
              })}
              <div className="grid grid-cols-2 gap-1">
                {SUPPORT_KINDS.map((k) => (
                  <button
                    key={k.id}
                    type="button"
                    title={k.hint}
                    className={btn}
                    onClick={() => {
                      sim.addSupport({ kind: k.id, faces: [] })
                      startPicking('support', setup.supports.length, [])
                    }}
                  >
                    <Plus size={11} aria-hidden /> {k.label}
                    {!k.solved && <span className="text-[11px] text-muted-foreground"> (check only)</span>}
                  </button>
                ))}
              </div>
            </>
          )}

          {step === 'loads' && (
            <>
              {setup.loads.map((l, i) => {
                const active = picking?.kind === 'load' && picking.index === i
                return (
                  <div
                    key={i}
                    className={cn('flex flex-col gap-1.5 rounded border px-2 py-1.5', active ? 'border-orange-500 bg-orange-500/10' : 'border-border/60')}
                    onMouseEnter={() => sim.setFocus({ kind: 'load', index: i })}
                    onMouseLeave={() => sim.setFocus(null)}
                  >
                    <div className="flex items-center gap-1.5">
                      <span className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ background: LOAD_COLOR }} aria-hidden />
                      <span className="font-medium">Force</span>
                      <span className="flex-1 text-muted-foreground">
                        {l.faces.length} face{l.faces.length === 1 ? '' : 's'}
                      </span>
                      <button type="button" className={btn} onClick={() => (active ? stopPicking() : startPicking('load', i, l.faces))}>
                        {active ? 'Done' : 'Pick faces'}
                      </button>
                      <button type="button" className="cad-focus p-1 text-muted-foreground hover:text-foreground" onClick={() => { if (active) stopPicking(); sim.removeLoad(i) }} aria-label="Remove load">
                        <Trash2 size={12} />
                      </button>
                    </div>
                    {active && <span className="text-[11px] text-orange-800 dark:text-orange-200">Click the faces the force acts on — each turns orange and the arrow follows.</span>}
                    <label className="flex items-center gap-1.5">
                      <span className="w-12 text-muted-foreground">Size</span>
                      <input name="force-n"
                        className={cn(field, 'h-6 w-20 font-mono')}
                        value={Number.isFinite(l.newtons) ? String(l.newtons) : ''}
                        onChange={(e) => {
                          const n = Number(e.target.value)
                          const g = l.given
                          const k = g && l.newtons > 0 ? n / l.newtons : 0
                          sim.updateLoad(i, g ? { newtons: n, given: [g[0] * k, g[1] * k, g[2] * k] } : { newtons: n })
                        }}
                        inputMode="decimal"
                        aria-label="Force (N)"
                      />
                      <span className="text-muted-foreground">N</span>
                    </label>
                    <div className="flex flex-wrap items-center gap-1">
                      <span className="w-12 text-muted-foreground">Way</span>
                      {DIRECTION_BUTTONS.filter((d) => (d.id !== 'into' && d.id !== 'out') || l.normal).map((d) => (
                        <button
                          key={d.id}
                          type="button"
                          title={d.title}
                          aria-pressed={l.direction === d.id}
                          onClick={() => sim.updateLoad(i, { direction: d.id })}
                          className={cn('cad-focus h-6 rounded border px-1.5 font-mono text-[11px]', l.direction === d.id ? 'border-orange-500 bg-orange-500/15 font-semibold' : 'border-border/70')}
                        >
                          {d.label}
                        </button>
                      ))}
                      {l.direction === 'given' && <span className="text-[11px] text-muted-foreground">as given ({l.given?.map((c) => Math.round(c * 10) / 10).join(', ')} N)</span>}
                      <button type="button" title="Reverse the force" onClick={() => sim.updateLoad(i, flipped(l))} className="cad-focus h-6 rounded border border-border/70 px-1.5 text-[11px]">
                        Flip
                      </button>
                    </div>
                    <div className="flex items-center gap-1">
                      <span className="w-12 text-muted-foreground">Spread</span>
                      {(['even', 'bearing'] as Spread[]).map((sp) => (
                        <button
                          key={sp}
                          type="button"
                          aria-pressed={l.spread === sp}
                          onClick={() => sim.updateLoad(i, { spread: sp })}
                          title={sp === 'even' ? 'The same everywhere on the faces' : 'A pin pressing on the half of a hole facing the pull'}
                          className={cn('cad-focus h-6 rounded border px-1.5 text-[11px]', l.spread === sp ? 'border-orange-500 bg-orange-500/15 font-semibold' : 'border-border/70')}
                        >
                          {sp === 'even' ? 'Even' : 'Pin in hole'}
                        </button>
                      ))}
                    </div>
                  </div>
                )
              })}
              <button
                type="button"
                className={btn}
                onClick={() => {
                  sim.addLoad({ faces: [], direction: 'into', newtons: 100, spread: 'even', normal: null })
                  startPicking('load', setup.loads.length, [])
                }}
              >
                <Plus size={11} aria-hidden /> Force
              </button>
            </>
          )}

          {step === 'material' && (
            <>
              <select name="material"
                className={field}
                aria-label="Material"
                value={library.find((x) => x.name === m.name)?.id ?? (m.name ? 'own' : '')}
                onChange={(e) => {
                  const chosen = library.find((x) => x.id === e.target.value)
                  if (chosen) sim.setMaterial(entryOf(chosen))
                  else sim.setMaterial({ name: 'My material', yieldBasis: null, source: '' })
                }}
              >
                <option value="" disabled>
                  Choose a material…
                </option>
                {library.map((x) => (
                  <option key={x.id} value={x.id}>
                    {x.name}
                  </option>
                ))}
                <option value="own">My own values…</option>
              </select>
              {m.name && (
                <div className="flex flex-col gap-1 rounded bg-muted/40 px-2 py-1.5">
                  <div className="grid grid-cols-[1fr_auto] items-center gap-x-1.5 gap-y-1">
                    <input name="young-s-modulus" className={cn(field, 'h-6 font-mono')} placeholder="Young's modulus" value={m.youngs_modulus_mpa} onChange={(e) => sim.setMaterial({ youngs_modulus_mpa: e.target.value, yieldBasis: null })} inputMode="decimal" aria-label="Young's modulus (MPa)" />
                    <span className="text-muted-foreground">MPa (E)</span>
                    <input name="poisson-ratio" className={cn(field, 'h-6 font-mono')} placeholder="Poisson ratio" value={m.poisson_ratio} onChange={(e) => sim.setMaterial({ poisson_ratio: e.target.value })} inputMode="decimal" aria-label="Poisson ratio" />
                    <span className="text-muted-foreground">ν</span>
                    <input name="yield-strength" className={cn(field, 'h-6 font-mono')} placeholder="Yield strength" value={m.yield_strength_mpa} onChange={(e) => sim.setMaterial({ yield_strength_mpa: e.target.value, yieldBasis: null })} inputMode="decimal" aria-label="Yield strength (MPa)" />
                    <span className="text-muted-foreground">MPa yield</span>
                  </div>
                  <input name="where-the-yield-comes-from" className={cn(field, 'h-6')} placeholder="Where the yield comes from" value={m.source} onChange={(e) => sim.setMaterial({ source: e.target.value })} aria-label="Source" />
                  {m.yieldBasis === 'specified_minimum' && (
                    <span className="text-[11px] text-emerald-700 dark:text-emerald-300">Yield is a specified minimum: fit for a design check.</span>
                  )}
                  {m.yieldBasis === 'typical' && (
                    <span className="text-[11px] text-amber-700 dark:text-amber-300">Yield is a TYPICAL value, not a design minimum: the safety factor will be indicative.</span>
                  )}
                  {!m.yieldBasis && <span className="text-[11px] text-muted-foreground">Your own values. No safety factor without a yield and its source.</span>}
                </div>
              )}
            </>
          )}

          {step === 'run' && !report && (
            <>
              <label className="flex items-center gap-2">
                <span className="w-24 text-muted-foreground">Safety factor ≥</span>
                <input name="e-g-2" className={cn(field, 'w-16 font-mono')} placeholder="e.g. 2" value={setup.minSafetyFactor} onChange={(e) => sim.setGoal(e.target.value)} inputMode="decimal" />
              </label>
              <AccuracySettings />
              <button type="button" className={primary} disabled={!study || !meshed || phase.kind === 'busy'} onClick={() => void run()}>
                <Play size={11} aria-hidden /> Run study
              </button>
              {!meshed && <span className="text-[11px] text-muted-foreground">Next: make the mesh (step 2).</span>}
              {meshed && !study && gaps.length > 0 && (
                <span className="text-[11px] text-muted-foreground">
                  Next: {gaps[0]}
                  {gaps.length > 1 ? ` (and ${gaps.length - 1} more)` : ''}
                </span>
              )}
            </>
          )}

          {phase.kind === 'busy' && <span className="text-[11px] font-medium text-sky-700 dark:text-sky-300">{phase.what}</span>}
          {phase.kind === 'unverified' && (
            <div className="flex flex-col gap-2 rounded bg-amber-500/10 px-2 py-2">
              <span>The part changed since it was last verified, and the kernel won't study unverified geometry.</span>
              <button type="button" onClick={() => void mesh(true)} className={cn(btn, 'self-start')}>
                Verify and make the mesh
              </button>
            </div>
          )}
          {phase.kind === 'can_move' && (
            <div className="flex flex-col gap-1 rounded bg-amber-500/10 px-2 py-2 text-amber-800 dark:text-amber-200">
              <span className="font-semibold">The part is not held: it can still move.</span>
              {phase.motions.map((mo) => (
                <span key={mo}>· {mo}</span>
              ))}
              <button type="button" className={cn(btn, 'self-start')} onClick={() => { setPhase({ kind: 'idle' }); sim.setStep('supports') }}>
                Back to supports
              </button>
            </div>
          )}
          {phase.kind === 'engine_missing' && <div className="rounded bg-amber-500/10 px-2 py-2">The mesher or solver is not installed on the server: {phase.reason}</div>}
          {phase.kind === 'refused' && <div className="rounded bg-red-500/10 px-2 py-2 text-red-700 dark:text-red-300">{phase.reason}</div>}

          {step !== 'run' && (
            <button type="button" className={cn(primary, 'mt-1 self-end')} disabled={!canNext[step]} onClick={next}>
              Next: {STEPS[index + 1]?.label} →
            </button>
          )}
        </div>

        {step === 'run' && report && <Results onEdit={() => { sim.setReport(null); sim.setStep('supports') }} />}
      </div>
    </section>
  )
}

const fmt = (v: number, unit: string) =>
  `${Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 1 ? v.toFixed(2) : v.toPrecision(3)} ${unit}`

/** The kernel's answer: the verdict, the numbers behind it, the checks it
 *  passed or failed, how the part is coloured, and which engines ran. */
function Results({ onEdit }: { onEdit: () => void }) {
  const report = useSimStore((s) => s.report)
  const shown = useSimStore((s) => s.shown)
  const show = useSimStore((s) => s.show)
  const deformed = useSimStore((s) => s.deformed)
  const setDeformed = useSimStore((s) => s.setDeformed)
  const [assumptionsOpen, setAssumptionsOpen] = useState(false)
  if (!report) return null
  const v = verdictView(report.verdict)
  const range = fieldRange(report.field, shown)
  const scale = deformScale(report.field)
  const gradient = `linear-gradient(to right, ${[0, 0.25, 0.5, 0.75, 1]
    .map((t) => `rgb(${stressColor(t).map((c) => Math.round(c * 255)).join(',')})`)
    .join(', ')})`

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className={cn('flex flex-col gap-0.5 px-3 py-2', STATE_STYLE[v.state].wash)}>
        <span className={cn('flex items-center gap-2 font-semibold', STATE_STYLE[v.state].text)}>
          <Mark state={v.state} />
          {v.title}
        </span>
        <span className="pl-5 text-[11px]">{v.detail}</span>
      </div>

      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-b border-border/70 px-3 py-2.5">
        <dt className="text-muted-foreground">Peak stress</dt>
        <dd className="text-right font-mono">{fmt(finalRun(report).peak_von_mises_mpa, 'MPa')}</dd>
        <dt className="text-muted-foreground">Largest movement</dt>
        <dd className="text-right font-mono">{fmt(finalRun(report).max_displacement_mm, 'mm')}</dd>
        <dt className="text-muted-foreground">{(report.refined?.length ?? 0) > 0 ? `Mesh (refined ×${report.refined?.length} at the peak)` : 'Mesh (proving)'}</dt>
        <dd className="text-right font-mono">{finalRun(report).mesh.nodes.toLocaleString()} nodes</dd>
      </dl>

      <div className="flex flex-col gap-1.5 border-b border-border/70 px-3 py-2.5">
        <div className="flex gap-1" role="radiogroup" aria-label="Colour the part by">
          {(['stress', 'displacement'] as const).map((k) => (
            <button
              key={k}
              type="button"
              role="radio"
              aria-checked={shown === k}
              onClick={() => show(k)}
              className={cn('cad-focus h-6 flex-1 rounded border border-border/70 font-medium', shown === k ? 'bg-foreground text-background' : 'bg-background')}
            >
              {k === 'stress' ? 'Stress' : 'Movement'}
            </button>
          ))}
        </div>
        <div className="h-2.5 rounded-sm" style={{ background: gradient }} aria-hidden />
        <div className="flex justify-between font-mono text-[11px] text-muted-foreground">
          <span>{fmt(range.min, range.unit)}</span>
          <span>{fmt(range.max, range.unit)}</span>
        </div>
        <label className="flex items-center gap-2">
          <input name="sim-panel-field-1" type="checkbox" checked={deformed} onChange={(e) => setDeformed(e.target.checked)} />
          <span>Show the bent shape{scale > 1 ? ` (movement ×${scale} so it can be seen)` : ''}</span>
        </label>
      </div>

      <ul className="flex flex-col gap-1 border-b border-border/70 px-3 py-2.5" aria-label="Checks">
        {report.checks.map((c) => (
          <li key={c.name} className="flex flex-col">
            <span className="flex items-center gap-2 font-medium">
              <Mark state={c.passed ? 'pass' : 'fix'} />
              {c.name}
            </span>
            <span className="pl-5 text-[11px] text-muted-foreground">{c.detail}</span>
          </li>
        ))}
      </ul>

      <div className="flex flex-col gap-1 px-3 py-2.5 text-[11px] text-muted-foreground">
        <span>Mesher: {engineLabel(report.engines.mesher)}</span>
        <span>Solver: {engineLabel(report.engines.solver)}</span>
        {report.assumptions.length > 0 && (
          <button type="button" className="cad-focus self-start underline" onClick={() => setAssumptionsOpen((o) => !o)}>
            {assumptionsOpen ? 'Hide' : 'Show'} the {report.assumptions.length} assumption{report.assumptions.length === 1 ? '' : 's'}
          </button>
        )}
        {assumptionsOpen && (
          <ul className="flex flex-col gap-0.5">
            {report.assumptions.map((a) => (
              <li key={a}>· {a}</li>
            ))}
          </ul>
        )}
        <button type="button" onClick={onEdit} className="cad-focus mt-1 h-7 self-start rounded border border-border bg-background px-2.5 font-medium text-foreground">
          Change the set-up
        </button>
      </div>
    </div>
  )
}

/** The mesh step in full: how fine (a density slider, or an exact size),
 *  how it follows curves, where to be finer, what kind of element, and --
 *  once made -- how good it is. Any change clears the mesh on screen until
 *  it is made again, so the mesh approved is always the mesh shown. */
function MeshStep({
  extent,
  busy,
  canMesh,
  onMesh,
  picking,
  onPick,
  onStopPicking,
}: {
  extent: number | null
  busy: boolean
  canMesh: boolean
  onMesh: () => void
  picking: { kind: 'support' | 'load' | 'local'; index: number } | null
  onPick: (index: number, faces: number[]) => void
  onStopPicking: () => void
}) {
  const setup = useSimStore((s) => s.setup)
  const mesh = useSimStore((s) => s.mesh)
  const setMesh = useSimStore((s) => s.setMesh)
  const addLocal = useSimStore((s) => s.addLocal)
  const updateLocal = useSimStore((s) => s.updateLocal)
  const removeLocal = useSimStore((s) => s.removeLocal)
  const lastMade = useSimStore((s) => s.mesh?.fine ?? s.lastMeshFine)

  // The size the settings ask for: the exact size if typed, else the slider.
  const typed = Number(setup.meshSizeMm)
  const stop = extent && setup.meshSizeMm.trim() !== '' && typed > 0
    ? DENSITY.reduce((best, d, i) => (Math.abs(extent / d.divisor - typed) < Math.abs(extent / DENSITY[best].divisor - typed) ? i : best), 0)
    : 2
  const coarseSize = setup.meshSizeMm.trim() !== '' && typed > 0 ? typed : extent ? extent / DENSITY[2].divisor : null
  const fineSize = coarseSize ? coarseSize * 0.6 : null
  const estimate = lastMade && fineSize ? estimateNodes(lastMade, fineSize) : null
  const quality = mesh?.fine
  const verdict = (q: number | undefined) =>
    q === undefined ? null : q >= 0.3 ? { label: 'good', cls: 'text-emerald-700 dark:text-emerald-300' } : q >= 0.15 ? { label: 'acceptable', cls: 'text-amber-700 dark:text-amber-300' } : { label: 'poor', cls: 'text-red-700 dark:text-red-300' }

  return (
    <div className="flex flex-col gap-3">
      {/* How fine */}
      <div className="flex flex-col gap-1.5">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">How fine</span>
        <input name="mesh-density"
          type="range"
          min={0}
          max={DENSITY.length - 1}
          step={1}
          value={stop}
          disabled={!extent}
          onChange={(e) => {
            const d = DENSITY[Number(e.target.value)]
            if (extent) setMesh({ meshSizeMm: (Math.round((extent / d.divisor) * 100) / 100).toString() })
          }}
          aria-label="Mesh density"
          className="w-full accent-sky-600"
        />
        <div className="flex justify-between text-[11px] text-muted-foreground">
          <span>Coarse · faster</span>
          <span className="font-medium text-foreground">{DENSITY[stop].label}</span>
          <span>Fine · more accurate</span>
        </div>
        <label className="flex items-center gap-2">
          <span className="w-28 text-muted-foreground">Element size</span>
          <input name="element-size-mm"
            className={cn(field, 'w-20 font-mono')}
            placeholder={coarseSize ? coarseSize.toFixed(2) : 'auto'}
            value={setup.meshSizeMm}
            onChange={(e) => setMesh({ meshSizeMm: e.target.value })}
            inputMode="decimal"
            aria-label="Element size (mm)"
          />
          <span className="text-muted-foreground">mm</span>
        </label>
        <span className="text-[11px] text-muted-foreground">
          Two meshes are made: a checking mesh at this size and a proving mesh at 0.6× ({fineSize ? `${fineSize.toFixed(2)} mm` : '—'}); the stress is read from the proving mesh, and the two must agree.
          {estimate !== null && ` About ${estimate.toLocaleString()} nodes on the proving mesh.`}
        </span>
      </div>

      {/* Curves */}
      <div className="flex flex-col gap-1.5">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">On curves</span>
        <label className="flex items-center gap-2">
          <span className="w-28 text-muted-foreground">Elements per circle</span>
          <input name="12"
            className={cn(field, 'w-16 font-mono')}
            placeholder="12"
            value={setup.curveElements}
            onChange={(e) => setMesh({ curveElements: e.target.value })}
            inputMode="numeric"
            aria-label="Elements around a full circle"
          />
        </label>
        <span className="text-[11px] text-muted-foreground">Fillets and holes get smaller elements so a full circle has this many; 0 turns it off.</span>
      </div>

      {/* Finer here */}
      <div className="flex flex-col gap-1.5">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Finer here</span>
        {setup.local.map((l, i) => {
          const active = picking?.kind === 'local' && picking.index === i
          return (
            <div key={i} className={cn('flex flex-col gap-1 rounded border px-2 py-1.5', active ? 'border-sky-500 bg-sky-500/10' : 'border-border/60')}>
              <div className="flex items-center gap-1.5">
                <span className="flex-1">
                  {l.faces.length} face{l.faces.length === 1 ? '' : 's'}
                </span>
                <input name="element-size-near-these-faces-mm"
                  className={cn(field, 'h-6 w-16 font-mono')}
                  value={l.sizeMm}
                  onChange={(e) => updateLocal(i, { sizeMm: e.target.value })}
                  inputMode="decimal"
                  aria-label="Element size near these faces (mm)"
                />
                <span className="text-muted-foreground">mm</span>
                <button type="button" className={btn} onClick={() => (active ? onStopPicking() : onPick(i, l.faces))}>
                  {active ? 'Done' : 'Pick faces'}
                </button>
                <button
                  type="button"
                  className="cad-focus p-1 text-muted-foreground hover:text-foreground"
                  onClick={() => {
                    if (active) onStopPicking()
                    removeLocal(i)
                  }}
                  aria-label="Remove"
                >
                  <Trash2 size={12} />
                </button>
              </div>
              {active && <span className="text-[11px] text-sky-800 dark:text-sky-200">Click the faces to mesh finer -- a fillet, a hole edge, where the stress gathers.</span>}
            </div>
          )
        })}
        <button
          type="button"
          className={btn}
          onClick={() => {
            const n = setup.local.length
            addLocal({ faces: [], sizeMm: fineSize ? (Math.round(fineSize * 0.4 * 100) / 100).toString() : '1' })
            onPick(n, [])
          }}
        >
          <Plus size={11} aria-hidden /> Finer on faces
        </button>
      </div>

      {/* Element */}
      <div className="flex flex-col gap-1">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Element</span>
        <span>Quadratic tetrahedra (10 nodes)</span>
        <span className="text-[11px] text-muted-foreground">Curved edges and accurate stresses with few elements; mesher Gmsh (interim).</span>
      </div>

      <button type="button" className={btn} disabled={!canMesh || busy} onClick={onMesh}>
        {mesh ? 'Make the mesh again' : 'Make the mesh'}
      </button>

      {/* What was made */}
      {mesh && quality && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 rounded bg-muted/40 px-2 py-1.5">
          <dt className="text-muted-foreground">Proving mesh</dt>
          <dd className="text-right font-mono">
            {mesh.fine.nodes.toLocaleString()} nodes · {mesh.fine.elements.toLocaleString()} elements
          </dd>
          <dt className="text-muted-foreground">Checking mesh</dt>
          <dd className="text-right font-mono">
            {mesh.coarse.nodes.toLocaleString()} nodes · {mesh.coarse.elements.toLocaleString()} elements
          </dd>
          <dt className="text-muted-foreground">Element size</dt>
          <dd className="text-right font-mono">
            {mesh.coarse.size_mm.toFixed(2)} → {mesh.fine.size_mm.toFixed(2)} mm
          </dd>
          {quality.quality_min !== undefined && (
            <>
              <dt className="text-muted-foreground" title="Normalised mean ratio: 1 = a perfect tetrahedron, toward 0 = a flat sliver">Element shape</dt>
              <dd className="text-right font-mono">
                worst {quality.quality_min.toFixed(2)} · average {(quality.quality_mean ?? 0).toFixed(2)}{' '}
                <span className={verdict(quality.quality_min)?.cls}>({verdict(quality.quality_min)?.label})</span>
              </dd>
              <dt className="text-muted-foreground">Poor elements</dt>
              <dd className="text-right font-mono">
                {(quality.poor_elements ?? 0).toLocaleString()}
                {mesh.fine.elements > 0 && ` (${(((quality.poor_elements ?? 0) / mesh.fine.elements) * 100).toFixed(2)} %)`}
              </dd>
            </>
          )}
          <dt className="text-muted-foreground" title="Measured against the part's display triangles: an upper bound -- the mesh nodes lie on the exact surface">Fits the part within</dt>
          <dd className="text-right font-mono">{mesh.fine.boundary_deviation_mm.toPrecision(2)} mm</dd>
          <dt className="text-muted-foreground">Made in</dt>
          <dd className="text-right font-mono">{(mesh.coarse.mesher_seconds + mesh.fine.mesher_seconds).toFixed(1)} s</dd>
        </dl>
      )}
      {!mesh && lastMade && <span className="text-[11px] text-amber-700 dark:text-amber-300">The settings changed: make the mesh again to see it.</span>}
    </div>
  )
}

/** How closely the meshes must agree for the answer to stand, and how far
 *  the kernel may refine at the peak to get there. */
function AccuracySettings() {
  const setup = useSimStore((s) => s.setup)
  const setAccuracy = useSimStore((s) => s.setAccuracy)
  const setMesh = useSimStore((s) => s.setMesh)
  return (
    <details className="rounded border border-border/60 px-2 py-1.5">
      <summary className="cursor-pointer text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Accuracy</summary>
      <div className="mt-2 flex flex-col gap-1.5">
        <label className="flex items-center gap-2">
          <span className="w-40 text-muted-foreground">Peak stress agrees within</span>
          <input name="5" className={cn(field, 'h-6 w-14 font-mono')} placeholder="5" value={setup.stressPct} onChange={(e) => setAccuracy({ stressPct: e.target.value })} inputMode="decimal" />
          <span className="text-muted-foreground">%</span>
        </label>
        <label className="flex items-center gap-2">
          <span className="w-40 text-muted-foreground">Movement agrees within</span>
          <input name="2" className={cn(field, 'h-6 w-14 font-mono')} placeholder="2" value={setup.displacementPct} onChange={(e) => setAccuracy({ displacementPct: e.target.value })} inputMode="decimal" />
          <span className="text-muted-foreground">%</span>
        </label>
        <label className="flex items-center gap-2">
          <input name="sim-panel-field-2" type="checkbox" checked={setup.refineAtPeak} onChange={(e) => setMesh({ refineAtPeak: e.target.checked })} />
          <span>Refine at the peak until it settles</span>
        </label>
        {setup.refineAtPeak && (
          <>
            <label className="flex items-center gap-2 pl-5">
              <span className="w-32 text-muted-foreground">At most</span>
              <input name="3" className={cn(field, 'h-6 w-14 font-mono')} placeholder="3" value={setup.maxRefinements} onChange={(e) => setMesh({ maxRefinements: e.target.value })} inputMode="numeric" />
              <span className="text-muted-foreground">refinements</span>
            </label>
            <label className="flex items-center gap-2 pl-5">
              <span className="w-32 text-muted-foreground">Up to</span>
              <input name="150000" className={cn(field, 'h-6 w-20 font-mono')} placeholder="150000" value={setup.nodeBudget} onChange={(e) => setMesh({ nodeBudget: e.target.value })} inputMode="numeric" />
              <span className="text-muted-foreground">nodes</span>
            </label>
          </>
        )}
        <span className="text-[11px] text-muted-foreground">
          Tighter limits and more refinement take longer; a peak that never settles is a sharp corner -- round it.
        </span>
      </div>
    </details>
  )
}