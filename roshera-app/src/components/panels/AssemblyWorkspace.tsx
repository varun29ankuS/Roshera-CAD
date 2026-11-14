/**
 * The Assembly workspace, on the certified assembly (lib/assembly-doc):
 * the document's parts placed as instances, joined by mates anchored on
 * their faces, and the kernel's own answers -- what each part is still free
 * to do, and whether anything touches through that motion. Laid out like
 * the Part workspace (Varun 2026-11-14, the approved mockup): the viewport
 * is the work; one floating card holds the assembly; the Blackboard is the
 * column on the right.
 */
import { useEffect, useMemo, useState } from 'react'
import { ChevronDown, Plus, RefreshCw, Shield, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { CADViewport } from '@/components/viewport/CADViewport'
import { ViewBar } from '@/components/viewport/ViewBar'
import { useSceneStore } from '@/stores/scene-store'
import { useAssemblyDocStore } from '@/stores/assembly-doc-store'
import { INSTANCE_PREFIX } from '@/lib/assembly-pose'
import {
  addFaceConnector,
  addInstance,
  addMate,
  assemblyStatus,
  createAssembly,
  freedomLabel,
  mateGlyph,
  mateKindName,
  removeInstance,
  removeMate,
  solveAssembly,
  type Constrainment,
  type MateKind,
} from '@/lib/assembly-doc'

const TONE = {
  positive: 'border-emerald-600/40 text-emerald-700 dark:text-emerald-400',
  info: 'border-sky-600/40 text-sky-700 dark:text-sky-300',
  caution: 'border-amber-500/50 text-amber-700 dark:text-amber-400',
  negative: 'border-red-500/50 text-red-700 dark:text-red-400',
  muted: 'border-border text-muted-foreground',
} as const

/** The joints the mate tool offers, as icons (the kernel's mate kinds). */
const JOINTS: { kind: MateKind; label: string; hint: string }[] = [
  { kind: 'Fastened', label: 'Fastened', hint: 'Locked together: no motion' },
  { kind: { Revolute: { limits: null } }, label: 'Revolute', hint: 'Turns about the axis' },
  { kind: { Slider: { limits: null } }, label: 'Slider', hint: 'Slides along the axis' },
  { kind: { Cylindrical: { rot_limits: null, trans_limits: null } }, label: 'Cylindrical', hint: 'Turns and slides on the axis' },
  { kind: 'Planar', label: 'Planar', hint: 'Slides and turns on the face' },
  { kind: 'Ball', label: 'Ball', hint: 'Turns about a point' },
]

type Pick = { instanceId: string; faceId: number }
type MateStep = { step: 'a' } | { step: 'b'; a: Pick } | { step: 'kind'; a: Pick; b: Pick } | { step: 'busy' }

function Section({ title, count, action, children }: { title: string; count?: number; action?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section className="border-t border-border/70">
      <div className="flex items-center gap-1.5 px-3 pb-1 pt-2">
        <span className="text-[10.5px] font-medium uppercase tracking-[0.08em] text-muted-foreground">{title}</span>
        {count !== undefined && <span className="text-[10.5px] text-muted-foreground">{count}</span>}
        <span className="flex-1" />
        {action}
      </div>
      <div className="pb-2">{children}</div>
    </section>
  )
}

function IconButton({ label, onClick, disabled, children }: { label: string; onClick: () => void; disabled?: boolean; children: React.ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="cad-focus rounded border border-border p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40"
    >
      {children}
    </button>
  )
}

export function AssemblyWorkspace() {
  const { ids, names, activeId, summary, dof, interference, checking, error } = useAssemblyDocStore()
  const { refreshList, open, reload, check, setError } = useAssemblyDocStore.getState()
  const objects = useSceneStore((s) => s.objects)
  const subSelections = useSceneStore((s) => s.subElementSelections)
  const [switcherOpen, setSwitcherOpen] = useState(false)
  const [addOpen, setAddOpen] = useState(false)
  const [mate, setMate] = useState<MateStep | null>(null)
  const [busy, setBusy] = useState(false)

  // The list on entry; open the first assembly when none is open.
  useEffect(() => {
    void refreshList()
  }, [refreshList])
  useEffect(() => {
    if (!activeId && ids.length > 0) void open(ids[0])
  }, [activeId, ids, open])

  // The document's parts (what an instance can be of).
  const parts = useMemo(
    () => [...objects.values()].filter((o) => typeof o.analyticalGeometry?.solidId === 'number'),
    [objects],
  )
  const instanceName = (id: string) => summary?.instances.find((i) => i.id === id)?.name ?? 'part'
  const constrainmentOf = (id: string): Constrainment | null =>
    dof?.instances.find((i) => i.instance_id === id)?.constrainment ?? null
  const status = assemblyStatus(dof?.constrainedness ?? null)

  const act = async (fn: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      await reload()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  // The mate tool: pick a face on one part, then on another, then a joint.
  useEffect(() => {
    if (!mate || (mate.step !== 'a' && mate.step !== 'b')) return
    const pick = subSelections.find((s) => s.type === 'face' && s.objectId.startsWith(INSTANCE_PREFIX))
    if (!pick) return
    const p: Pick = { instanceId: pick.objectId.slice(INSTANCE_PREFIX.length), faceId: pick.index }
    useSceneStore.getState().clearSubElementSelections?.()
    if (mate.step === 'a') setMate({ step: 'b', a: p })
    else if (p.instanceId !== mate.a.instanceId) setMate({ step: 'kind', a: mate.a, b: p })
  }, [mate, subSelections])

  const startMate = () => {
    useSceneStore.getState().setSelectionMode('face')
    useSceneStore.getState().clearSubElementSelections?.()
    setMate({ step: 'a' })
  }
  const endMate = () => {
    setMate(null)
    useSceneStore.getState().setSelectionMode('object')
  }
  const makeMate = async (kind: MateKind) => {
    if (!activeId || !mate || mate.step !== 'kind') return
    const { a, b } = mate
    setMate({ step: 'busy' })
    await act(async () => {
      const ca = await addFaceConnector(activeId, a.instanceId, a.faceId)
      const cb = await addFaceConnector(activeId, b.instanceId, b.faceId)
      await addMate(activeId, kind, ca.connector_id, cb.connector_id)
      await solveAssembly(activeId)
    })
    endMate()
    void check()
  }

  const clearance = (() => {
    if (!interference) return null
    if (!interference.computed) return { tone: 'muted' as const, text: interference.refused_reason ?? 'not checked' }
    if (interference.static_pairs.length > 0) return { tone: 'negative' as const, text: `✗ ${interference.static_pairs.length} clash${interference.static_pairs.length === 1 ? '' : 'es'}` }
    const sweeps = interference.sweeps
    if (sweeps.some((s) => !s.clear)) return { tone: 'negative' as const, text: '✗ contact during the motion' }
    const min = Math.min(...sweeps.map((s) => s.min_certified_clearance ?? Infinity))
    return {
      tone: 'positive' as const,
      text: sweeps.length > 0 && Number.isFinite(min) ? `✓ clear through the motion · min ${Number(min.toFixed(3))} mm` : '✓ no clashes',
    }
  })()

  return (
    <div className="relative min-w-0 flex-1 overflow-hidden">
      <CADViewport />
      <div className="pointer-events-none absolute left-[18rem] right-2 top-2 z-20 flex justify-center">
        <ViewBar />
      </div>

      <section
        aria-label="Assembly"
        className="absolute left-2 top-2 z-20 flex max-h-[calc(100%-1rem)] w-[17rem] flex-col overflow-hidden rounded-lg border border-border bg-card/95 text-xs shadow-sm"
      >
        {/* Header: which assembly, its verdict, new. */}
        <div className="relative flex items-center gap-1.5 border-b border-border px-2 py-1.5">
          <button
            type="button"
            onClick={() => setSwitcherOpen((o) => !o)}
            className="cad-focus flex min-w-0 flex-1 items-center gap-1 rounded px-1 py-0.5 text-left hover:bg-accent"
          >
            <span className="truncate text-[12.5px] font-semibold">{summary?.name ?? (ids.length ? 'Assembly' : 'No assembly')}</span>
            <ChevronDown size={12} className="shrink-0 text-muted-foreground" />
          </button>
          {summary && (
            <span className={cn('shrink-0 rounded border px-1.5 py-px text-[11px] font-medium', TONE[status.tone])} title="What the kernel says the assembly can do">
              {status.text}
            </span>
          )}
          <IconButton
            label="New assembly"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                const { id } = await createAssembly(`Assembly ${ids.length + 1}`)
                await refreshList()
                await open(id)
              })
            }
          >
            <Plus size={13} />
          </IconButton>
          {switcherOpen && ids.length > 0 && (
            <div className="absolute left-2 top-full z-50 mt-1 w-56 rounded-md border border-border bg-popover py-1 shadow-lg">
              {ids.map((id) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => {
                    setSwitcherOpen(false)
                    void open(id)
                  }}
                  className={cn('cad-focus block w-full truncate px-3 py-1 text-left hover:bg-accent', id === activeId && 'font-medium')}
                >
                  {id === activeId ? '✓ ' : ''}
                  {names[id] ?? 'Assembly'}
                </button>
              ))}
            </div>
          )}
        </div>

        {!summary ? (
          <p className="px-3 py-3 text-muted-foreground">
            {ids.length === 0 ? 'No assembly yet — + makes one from this document’s parts.' : 'Opening…'}
          </p>
        ) : (
          <div className="min-h-0 overflow-y-auto [scrollbar-width:thin]">
            {/* Parts placed in the assembly, and what each may still do. */}
            <Section
              title="Parts"
              count={summary.instances.length}
              action={
                <div className="relative">
                  <IconButton label="Add a part" disabled={busy || parts.length === 0} onClick={() => setAddOpen((o) => !o)}>
                    <Plus size={13} />
                  </IconButton>
                  {addOpen && (
                    <div className="absolute right-0 top-full z-50 mt-1 w-44 rounded-md border border-border bg-popover py-1 shadow-lg">
                      {parts.map((p) => (
                        <button
                          key={p.id}
                          type="button"
                          onClick={() => {
                            setAddOpen(false)
                            void act(() => addInstance(summary.id, p.id, p.name))
                          }}
                          className="cad-focus block w-full truncate px-3 py-1 text-left hover:bg-accent"
                        >
                          {p.name}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              }
            >
              {summary.instances.map((inst, n) => {
                const c = constrainmentOf(inst.id)
                const grounded = n === 0
                const free = c?.status === 'mobile'
                return (
                  <div key={inst.id} className="group flex items-center gap-2 px-3 py-1 hover:bg-accent/50">
                    <span className="w-4 shrink-0 text-center text-[13px]" title={grounded ? 'Grounded: never moves' : free ? 'Free to move' : 'Located'}>
                      {grounded ? '⚓' : free ? '⟳' : '■'}
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-medium">{inst.name ?? 'Part'}</div>
                      <div className={cn('truncate text-[11px]', free ? 'text-sky-700 dark:text-sky-300' : 'text-muted-foreground')}>
                        {c ? freedomLabel(c, grounded) : '—'}
                      </div>
                    </div>
                    <span
                      className={inst.sound ? 'text-emerald-600' : 'text-red-600'}
                      title={inst.sound === null ? 'Its part cannot be found' : inst.sound ? 'Part is sound' : 'Part is not sound'}
                    >
                      {inst.sound ? '✓' : '✗'}
                    </span>
                    <button
                      type="button"
                      aria-label={`Remove ${inst.name ?? 'part'}`}
                      title="Remove from the assembly"
                      disabled={busy}
                      onClick={() => void act(() => removeInstance(summary.id, inst.id))}
                      className="cad-focus rounded p-0.5 text-muted-foreground opacity-0 hover:text-destructive group-hover:opacity-100"
                    >
                      <X size={12} />
                    </button>
                  </div>
                )
              })}
            </Section>

            {/* Mates: what joins what, as a glyph. */}
            <Section
              title="Mates"
              count={summary.mates?.length ?? 0}
              action={
                <IconButton label="Add a mate: pick a face on each part" disabled={busy || summary.instances.length < 2} onClick={startMate}>
                  <Plus size={13} />
                </IconButton>
              }
            >
              {(summary.mates ?? []).map((m) => (
                <div key={m.id} className="group flex items-center gap-2 px-3 py-1 hover:bg-accent/50" title={mateKindName(m.kind)}>
                  <span className="w-4 shrink-0 text-center text-[14px]">{mateGlyph(m.kind)}</span>
                  <span className="min-w-0 flex-1 truncate">
                    {instanceName(m.a_instance)} <span className="text-muted-foreground">·</span> {instanceName(m.b_instance)}
                  </span>
                  <span className="text-[11px] text-muted-foreground">{mateKindName(m.kind)}</span>
                  <button
                    type="button"
                    aria-label="Remove the mate"
                    title="Remove the mate"
                    disabled={busy}
                    onClick={() => void act(async () => { await removeMate(summary.id, m.id); await solveAssembly(summary.id) })}
                    className="cad-focus rounded p-0.5 text-muted-foreground opacity-0 hover:text-destructive group-hover:opacity-100"
                  >
                    <X size={12} />
                  </button>
                </div>
              ))}
              {mate && (
                <div className="mx-2 mt-1 rounded-md border border-[var(--cad-selected)] bg-[var(--cad-selected-wash)] p-2">
                  {mate.step === 'a' && <div>① Click a face on the first part.</div>}
                  {mate.step === 'b' && <div>② Now a face on {instanceName(mate.a.instanceId)}'s partner.</div>}
                  {mate.step === 'kind' && (
                    <>
                      <div className="mb-1.5">③ How do {instanceName(mate.a.instanceId)} and {instanceName(mate.b.instanceId)} move?</div>
                      <div className="grid grid-cols-3 gap-1">
                        {JOINTS.map((j) => (
                          <button
                            key={j.label}
                            type="button"
                            title={j.hint}
                            onClick={() => void makeMate(j.kind)}
                            className="cad-focus flex flex-col items-center rounded border border-border bg-card py-1 hover:border-[var(--cad-selected)]"
                          >
                            <span className="text-[15px] leading-none">{mateGlyph(j.kind)}</span>
                            <span className="mt-0.5 text-[10.5px]">{j.label}</span>
                          </button>
                        ))}
                      </div>
                    </>
                  )}
                  {mate.step === 'busy' && <div>Solving…</div>}
                  {mate.step !== 'busy' && (
                    <button type="button" onClick={endMate} className="cad-focus mt-1.5 text-[11px] text-muted-foreground hover:text-foreground">
                      Cancel
                    </button>
                  )}
                </div>
              )}
            </Section>

            {/* The kernel's checks, one click each. */}
            <Section title="Checks">
              <div className="flex items-center gap-2 px-3">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void act(() => solveAssembly(summary.id))}
                  className="cad-focus flex items-center gap-1 rounded border border-border px-2 py-1 hover:bg-accent"
                  title="Solve the mates and place the parts"
                >
                  <RefreshCw size={12} /> Solve
                </button>
                <button
                  type="button"
                  disabled={checking}
                  onClick={() => void check()}
                  className="cad-focus flex items-center gap-1 rounded border border-border px-2 py-1 hover:bg-accent"
                  title="Check for clashes, through every free motion"
                >
                  <Shield size={12} /> {checking ? 'Checking…' : 'Clearance'}
                </button>
              </div>
              {clearance && (
                <div className={cn('mx-3 mt-1.5 rounded border px-2 py-1 text-[11px]', TONE[clearance.tone])}>{clearance.text}</div>
              )}
            </Section>
          </div>
        )}
      </section>

      {error && (
        <div role="alert" className="absolute bottom-3 left-1/2 z-30 flex -translate-x-1/2 items-center gap-2 rounded-md border border-destructive/30 bg-card px-3 py-1.5 text-xs shadow-md">
          <span className="text-red-700 dark:text-red-300">{error}</span>
          <button type="button" aria-label="Dismiss" onClick={() => setError(null)} className="cad-focus text-muted-foreground">
            <X size={12} />
          </button>
        </div>
      )}
    </div>
  )
}
