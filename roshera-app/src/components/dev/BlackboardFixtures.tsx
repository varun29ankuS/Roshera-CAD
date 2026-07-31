import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowLeft, Pause, Play, RotateCcw } from 'lucide-react'
import { Blackboard } from '@/components/panels/Blackboard'
import { MessageMarkdown } from '@/components/panels/MessageMarkdown'
import { StreamingLineText } from '@/components/panels/StreamingLineText'
import { RevealContext } from '@/components/panels/cards/reveal-context'
import {
  useBlackboardStore,
  type AgentAttention,
} from '@/stores/blackboard-store'
import {
  GDT_CHARACTERISTICS,
  GDT_MODIFIERS,
  GDT_TEXT_NOTATIONS,
  type GdtGroup,
} from '@/lib/gdt-symbols'
import { cn } from '@/lib/utils'

/**
 * BLACKBOARD FIXTURES — dev harness (#/blackboard-fixtures)
 * =========================================================
 * The backend is down and `npm run dev` is not authorised from agent
 * sessions, so the states below cannot be staged live. This page renders
 * every Blackboard state a human needs to verify at next boot, in one
 * place, through the REAL pipeline (fence → schema validation → typed card;
 * streaming → buffered segmentation) — nothing here bypasses the production
 * render path. Payload SHAPES mirror the verified wire types
 * (dfm/report.rs, branches.rs, gdt.ts, mcp core.ts); the NUMBERS are
 * illustrative, not measured from a live part.
 *
 * Nothing on this page writes to the Blackboard store's notebook — the
 * embedded live panel shows the real document notebook untouched; fixture
 * lines are rendered locally.
 */

const FENCE = '```'

function card(kind: string, payload: unknown): string {
  return `${FENCE}roshera:${kind}\n${JSON.stringify(payload, null, 2)}\n${FENCE}`
}

// ── Fixture payloads (shapes verified; numbers illustrative) ──────────

const FCF_CERTIFIED_IN_SPEC = card('fcf', {
  characteristic: 'position',
  tolerance: { value_mm: 0.1, diameter: true },
  datums: ['A', 'B'],
  standard: 'asme-y14.5-2018',
  verdict: {
    conforms: 'in_spec',
    tolerance_mm: 0.1,
    measured_mm: 0.031,
    fit_residual_mm: 0.00001,
    datum_statuses: [
      { label: 'A', status: 'live' },
      { label: 'B', status: 'live' },
    ],
    frame: { origin: [0, 0, 0], derivation: 'axis B ∩ plane A' },
  },
})

const FCF_CERTIFIED_OUT_OF_SPEC = card('fcf', {
  characteristic: 'perpendicularity',
  tolerance: { value_mm: 0.05 },
  datums: ['A'],
  standard: 'asme-y14.5-2018',
  verdict: {
    conforms: 'out_of_spec',
    tolerance_mm: 0.05,
    measured_mm: 0.142,
    fit_residual_mm: 0.0001,
    datum_statuses: [{ label: 'A', status: 'live' }],
  },
})

const FCF_NOT_EVALUABLE = card('fcf', {
  characteristic: 'perpendicularity',
  tolerance: { value_mm: 0.05 },
  datums: ['A'],
  verdict: {
    conforms: 'not_evaluable',
    tolerance_mm: 0.05,
    reason: "datum 'A' is dangling — its source face was consumed by a later operation",
    datum_statuses: [{ label: 'A', status: 'dangling' }],
  },
})

const FCF_DESIGN_INTENT = card('fcf', {
  characteristic: 'cylindricity',
  tolerance: { value_mm: 0.02 },
  standard: 'asme-y14.5-2018',
  note: 'Bore form for the bushing press fit — stated as intent; cylindricity is outside the certified set.',
})

const FCF_MODIFIER_UNCERTIFIED = card('fcf', {
  characteristic: 'position',
  tolerance: { value_mm: 0.1, diameter: true, modifier: 'mmc' },
  datums: ['A', 'B', 'C'],
  standard: 'asme-y14.5-2018',
  note: 'Bonus tolerance intended at MMC — kernel evaluation is RFS-only, so this frame is not a certificate.',
})

const FCF_DIALECT_DEPRECATED = card('fcf', {
  characteristic: 'concentricity',
  tolerance: { value_mm: 0.05, diameter: true },
  datums: ['A'],
  standard: 'asme-y14.5-2018',
})

const DFM_VIOLATION = card('dfm', {
  rule: 'fdm.min_wall',
  verdict: {
    kind: 'violation',
    witnesses: [41],
    measured: {
      value: 1.2,
      derivation: { kind: 'analytic', surface_type: 'plane', method: 'plane-pair distance' },
    },
    limit: {
      value: 1.6,
      derivation: {
        kind: 'analytic',
        surface_type: 'plane',
        method: '4x perimeter width at 0.4 mm nozzle',
      },
    },
  },
  provenance: {
    kind: 'shop_practice',
    note: '4 perimeter widths at a 0.4 mm nozzle; practice-derived, no governing standard',
  },
  unit: 'mm',
})

const DFM_UNVERIFIABLE_BOUND = card('dfm', {
  rule: 'fdm.min_wall',
  verdict: {
    kind: 'unverifiable',
    regions: [12],
    reason: {
      kind: 'bound_not_separating',
      lo: 0.78,
      hi: 0.86,
      limit: 0.8,
      refinement_depth: 6,
      converged: false,
    },
  },
  provenance: {
    kind: 'shop_practice',
    note: '2x nozzle wall floor; practice-derived, no governing standard',
  },
  unit: 'mm',
})

const DFM_PASS = card('dfm', {
  rule: 'im.min_draft',
  verdict: {
    kind: 'pass',
    margin: {
      value: 0.6,
      derivation: {
        kind: 'bounded_analytic',
        method: 'cone half-angle vs pull direction',
        refinement_depth: 3,
        converged: true,
      },
      bound: { lo: 0.6, hi: 0.72 },
    },
  },
  provenance: {
    kind: 'handbook',
    citation: 'Boothroyd & Dewhurst, Product Design for Manufacture and Assembly',
  },
  unit: '°',
})

const REFUSAL = card('refusal', {
  reason:
    'face 17 is a freeform blend — datum designation requires a planar face (datum plane) or a cylindrical face (datum axis)',
  subject: 'datum A on the arm underside',
  source: 'kernel',
  options: [
    'Designate the adjacent planar seat face as datum A instead',
    'Replace the blend with a conical chamfer, which qualifies as a datum feature',
  ],
})

const MERGE_CONFLICT = card('merge', {
  success: false,
  merged_into: 'main',
  strategy: 'three-way',
  events_merged: 0,
  source: 'wall-bracket-rib',
  conflicts: [
    {
      subject: 'solid:0',
      conflict_type: 'concurrent_modification',
      summary:
        'solid:0 was modified on both branches: transform_solid (agent:claude, seq 14) vs boolean_operation (user:varun, seq 9)',
      source_event: {
        id: '4c2a7e9d-1b3f-4a58-9c62-8e5d1f0a7b34',
        sequence_number: 14,
        timestamp: '2026-07-31T09:41:22Z',
        operation_type: 'transform_solid',
        author: 'agent:claude',
      },
      target_event: {
        id: 'a91d4e02-77c5-4f1b-8d3a-52b9c60e14f8',
        sequence_number: 9,
        timestamp: '2026-07-31T09:38:05Z',
        operation_type: 'boolean_operation',
        author: 'user:varun',
      },
    },
  ],
  statistics: {
    events_merged: 0,
    conflicts_count: 1,
    auto_resolved: 0,
    entities_affected: 1,
    duration_ms: 4,
  },
})

const MERGE_CLEAN = card('merge', {
  success: true,
  merged_into: 'main',
  strategy: 'fast-forward',
  events_merged: 12,
  source: 'm6-clearance-bores',
  conflicts: [],
  statistics: {
    events_merged: 12,
    conflicts_count: 0,
    auto_resolved: 0,
    entities_affected: 3,
    duration_ms: 7,
  },
})

const SOUNDNESS_FULL = card('soundness', {
  part: 'wall bracket',
  sound: true,
  brep_valid: true,
  watertight: true,
  manifold: true,
  self_intersection_free: true,
  construction_consistent: true,
  labels_consistent: true,
  tessellation_clean: true,
  mesh_quality_clean: true,
  eyes_consistent: true,
  euler_characteristic: 2,
  open_edges: 0,
  nonmanifold_edges: 0,
  face_count: 11,
  volume: 12406.9,
})

const SOUNDNESS_UNSOUND = card('soundness', {
  part: 'bore fitting',
  sound: false,
  brep_valid: true,
  watertight: false,
  manifold: true,
  self_intersection_free: true,
  construction_consistent: true,
  labels_consistent: true,
  tessellation_clean: true,
  mesh_quality_clean: true,
  eyes_consistent: false,
  euler_characteristic: 1,
  open_edges: 6,
  nonmanifold_edges: 0,
  face_count: 14,
  volume: 8021.4,
  errors: ['6 open edges on face 9 — boundary loop does not close after the last fillet'],
})

const SOUNDNESS_PARTIAL = card('soundness', {
  part: 'flanged nozzle',
  sound: null,
  brep_valid: true,
  watertight: true,
  manifold: true,
  self_intersection_free: null,
  construction_consistent: null,
  labels_consistent: true,
  tessellation_clean: null,
  mesh_quality_clean: null,
  eyes_consistent: null,
  euler_characteristic: 2,
  open_edges: 0,
  face_count: 24,
})

const CARD_FIXTURES: Array<{ title: string; note: string; source: string }> = [
  {
    title: 'FCF — certified, in spec',
    note: 'Position ⌖ ⌀0.10 A|B with a kernel verdict: solid "kernel-certified · RFS" chip, measured value, DRF disclosure.',
    source: FCF_CERTIFIED_IN_SPEC,
  },
  {
    title: 'FCF — certified, out of spec',
    note: 'Perpendicularity ⊥ 0.05 A proven out of spec.',
    source: FCF_CERTIFIED_OUT_OF_SPEC,
  },
  {
    title: 'FCF — not evaluable (dangling datum)',
    note: 'The kernel reports honestly when a datum face was consumed.',
    source: FCF_NOT_EVALUABLE,
  },
  {
    title: 'FCF — design intent (uncertified characteristic)',
    note: 'Cylindricity ⌭ renders as proper notation but carries the dashed design-intent chip: notation is comprehensive, certification is not.',
    source: FCF_DESIGN_INTENT,
  },
  {
    title: 'FCF — design intent (modifier outside the schema)',
    note: 'Position with Ⓜ: a certified characteristic made uncertified by a material-condition modifier the kernel schema cannot express.',
    source: FCF_MODIFIER_UNCERTIFIED,
  },
  {
    title: 'FCF — dialect marker (Y14.5-2018 removal)',
    note: 'Concentricity ◎ under ASME Y14.5-2018 carries the removal marker; ISO retains coaxiality.',
    source: FCF_DIALECT_DEPRECATED,
  },
  {
    title: 'DFM — proven violation',
    note: 'fdm.min_wall: measured vs limit with derivations, witness faces, shop-practice provenance.',
    source: DFM_VIOLATION,
  },
  {
    title: 'DFM — unverifiable with proven bound',
    note: 'The enclosure [0.78, 0.86] straddles the 0.8 limit: refusal WITH the bound, never folded to a pass.',
    source: DFM_UNVERIFIABLE_BOUND,
  },
  {
    title: 'DFM — proven pass',
    note: 'Bounded-analytic margin with its enclosure and handbook provenance.',
    source: DFM_PASS,
  },
  {
    title: 'Typed refusal',
    note: 'A result, not an error: verbatim reason, calm styling, next actions.',
    source: REFUSAL,
  },
  {
    title: 'Merge — blocked with typed witnesses',
    note: 'Taxonomy verdict + both colliding events, rendered from the MergeView shape.',
    source: MERGE_CONFLICT,
  },
  {
    title: 'Merge — clean',
    note: 'Fast-forward fold with statistics.',
    source: MERGE_CLEAN,
  },
  {
    title: 'Soundness — full certificate',
    note: 'Every invariant proven; χ = 2, no open edges.',
    source: SOUNDNESS_FULL,
  },
  {
    title: 'Soundness — unsound (proven violations)',
    note: 'watertight and dual-eye consistency FAIL — the red cross, not just the green tick and the dashed not-run.',
    source: SOUNDNESS_UNSOUND,
  },
  {
    title: 'Soundness — partial (tri-state)',
    note: 'Cheap hot-path verdict: not-run invariants render dashed — a check that did not run is not a check that passed.',
    source: SOUNDNESS_PARTIAL,
  },
]

// ── Streaming simulation ──────────────────────────────────────────────

const STREAM_SCRIPT = [
  'Sizing the bracket root before cutting the bores. With the worst-case load ',
  '$F = 150\\,\\text{N}$ at the arm tip, the root bending stress is',
  '\n\n$$\\sigma = \\dfrac{Mc}{I} = \\dfrac{(150\\,\\text{N} \\cdot 180\\,\\text{mm})\\,c}{I} = 40.5\\ \\text{MPa}$$\n\n',
  'a hand calculation, not a kernel verdict — inside the 60 MPa allowable only with the thicker web. Checking the printed wall floor next.\n\n',
  DFM_VIOLATION,
  '\nThickening the web to 2.4 mm and re-running the pack.',
].join('')

function StreamingDemo() {
  const [pos, setPos] = useState(0)
  const [running, setRunning] = useState(true)
  const timerRef = useRef<number | null>(null)
  const complete = pos >= STREAM_SCRIPT.length

  useEffect(() => {
    if (!running || complete) return
    timerRef.current = window.setInterval(() => {
      setPos((p) => Math.min(p + 2 + Math.floor(Math.random() * 5), STREAM_SCRIPT.length))
    }, 24)
    return () => {
      if (timerRef.current !== null) window.clearInterval(timerRef.current)
    }
  }, [running, complete])

  const restart = useCallback(() => {
    setPos(0)
    setRunning(true)
  }, [])

  const text = STREAM_SCRIPT.slice(0, pos)
  return (
    <div>
      <div className="mb-2 flex items-center gap-1.5">
        <button onClick={restart} className="cad-icon-btn h-6 px-1.5 text-[11px]" title="Restart the stream">
          <RotateCcw size={11} /> <span className="ml-1">restart</span>
        </button>
        {!complete && (
          <>
            <button
              onClick={() => setRunning((r) => !r)}
              className="cad-icon-btn h-6 px-1.5 text-[11px]"
              title={running ? 'Pause' : 'Resume'}
            >
              {running ? <Pause size={11} /> : <Play size={11} />}
            </button>
            <button
              onClick={() => setPos(STREAM_SCRIPT.length)}
              className="cad-icon-btn h-6 px-1.5 text-[11px]"
              title="Jump to the settled state"
            >
              finish
            </button>
          </>
        )}
        <span className="text-[10px] text-muted-foreground">
          {complete
            ? 'settled — full text through the normal render path'
            : 'streaming — math and the card are withheld until complete, then typeset once'}
        </span>
      </div>
      <div className="rounded-md border border-border/60 bg-background/40 px-3 py-2 text-sm leading-relaxed">
        <RevealContext.Provider value={{ animate: true }}>
          {complete ? <MessageMarkdown content={text} /> : <StreamingLineText text={text} />}
        </RevealContext.Provider>
      </div>
    </div>
  )
}

// ── Symbol reference (doubles as the font-coverage check at boot) ─────

const GROUP_LABELS: Record<GdtGroup, string> = {
  form: 'Form',
  profile: 'Profile',
  orientation: 'Orientation',
  location: 'Location',
  runout: 'Runout',
}

function SymbolReference() {
  const groups: GdtGroup[] = ['form', 'profile', 'orientation', 'location', 'runout']
  return (
    <div className="space-y-4">
      {groups.map((g) => (
        <div key={g}>
          <div className="cad-panel-header border-none px-0 pb-1">{GROUP_LABELS[g]}</div>
          <div className="space-y-1">
            {GDT_CHARACTERISTICS.filter((c) => c.group === g).map((c) => (
              <div key={c.id} className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs">
                <span className="gdt-glyph w-8 text-center text-xl text-foreground">{c.glyph}</span>
                <span className="w-40 text-foreground/90">{c.name}</span>
                <span className="cad-readout w-20 text-muted-foreground">{c.codePoint}</span>
                {c.certified ? (
                  <span className="rounded border border-emerald-500/40 px-1.5 py-px text-[10px] uppercase tracking-wide text-emerald-400">
                    kernel-certified · RFS
                  </span>
                ) : (
                  <span className="rounded border border-dashed border-border px-1.5 py-px text-[10px] uppercase tracking-wide text-muted-foreground">
                    design intent
                  </span>
                )}
                {c.asme === 'removed-2018' && (
                  <span className="text-[10px] text-amber-400/90">
                    removed in ASME Y14.5-2018{c.isoName ? ` · ISO retains ${c.isoName.toLowerCase()}` : ''}
                  </span>
                )}
                {c.conventional && (
                  <span className="basis-full pl-11 text-[10px] text-muted-foreground/80">
                    {c.conventional}
                  </span>
                )}
              </div>
            ))}
          </div>
        </div>
      ))}

      <div>
        <div className="cad-panel-header border-none px-0 pb-1">Modifiers</div>
        <div className="space-y-1">
          {GDT_MODIFIERS.map((m) => (
            <div key={m.id} className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs">
              <span className="gdt-glyph w-8 text-center text-lg text-foreground">
                {m.glyph ?? m.fallback ?? '—'}
              </span>
              <span className="w-56 text-foreground/90">{m.name}</span>
              <span className="cad-readout w-20 text-muted-foreground">{m.codePoint ?? 'none'}</span>
              {m.glyph === null && (
                <span className="text-[10px] text-amber-400/80">
                  no Unicode character — deliberate fallback, not a lookalike
                </span>
              )}
              {m.dialect === 'iso-only' && (
                <span className="text-[10px] text-muted-foreground">ISO only</span>
              )}
              <span className="basis-full pl-11 text-[10px] text-muted-foreground/80">{m.meaning}</span>
            </div>
          ))}
          {GDT_TEXT_NOTATIONS.map((t) => (
            <div key={t.id} className="flex items-baseline gap-x-3 text-xs">
              <span className="cad-readout w-8 text-center text-sm text-foreground">{t.notation}</span>
              <span className="w-56 text-foreground/90">{t.name}</span>
              <span className="cad-readout w-20 text-muted-foreground">plain text</span>
            </div>
          ))}
        </div>
        <p className="mt-2 max-w-2xl text-[10px] leading-relaxed text-muted-foreground">
          No modifier is accepted by the kernel schema — a frame carrying one is design intent
          by definition (kernel evaluation is RFS-only). Basic dimensions, datum feature symbols
          and datum targets have no character representation; they are renderings (boxed value,
          boxed letter on a leader) drawn by the UI, deliberately not approximated with glyphs.
        </p>
      </div>
    </div>
  )
}

// ── Page ──────────────────────────────────────────────────────────────

function Section({ title, note, children }: { title: string; note?: string; children: React.ReactNode }) {
  return (
    <section className="mb-8">
      <h2 className="mb-0.5 text-sm font-medium text-foreground">{title}</h2>
      {note && <p className="mb-2 max-w-3xl text-[11px] leading-relaxed text-muted-foreground">{note}</p>}
      {children}
    </section>
  )
}

interface BlackboardFixturesProps {
  onExit: () => void
}

export function BlackboardFixtures({ onExit }: BlackboardFixturesProps) {
  const agentAttention = useBlackboardStore((s) => s.agentAttention)
  const setAgentAttention = useBlackboardStore((s) => s.setAgentAttention)
  const setPanel = useBlackboardStore((s) => s.setPanel)
  const [replayKey, setReplayKey] = useState(0)

  // The embedded panel must be open to demonstrate the attention split.
  useEffect(() => {
    setPanel(true)
    return () => setAgentAttention('idle')
  }, [setPanel, setAgentAttention])

  const attentions: AgentAttention[] = ['idle', 'writing', 'geometry']

  return (
    <div className="h-full w-full overflow-y-auto bg-background text-foreground select-text">
      <div className="mx-auto max-w-4xl px-6 py-6">
        <div className="mb-6 flex items-center gap-3">
          <button onClick={onExit} className="cad-icon-btn h-7 w-7" title="Back to workspace" aria-label="Back to workspace">
            <ArrowLeft size={14} />
          </button>
          <div>
            <h1 className="text-base font-medium">Blackboard fixtures</h1>
            <p className="text-[11px] text-muted-foreground">
              Every state in one place, rendered through the production pipeline. Payload shapes
              mirror the verified wire types; numbers are illustrative.
            </p>
          </div>
        </div>

        <Section
          title="Attention-following layout"
          note="The live Blackboard (your real notebook) against a stand-in viewport. Writing expands the board; geometry collapses it to a strip so the viewport takes the space. Drag the top grip to override — the override sticks until you double-click the grip or press its auto chip. The resize never gates geometry."
        >
          <div className="mb-2 flex items-center gap-1.5">
            {attentions.map((a) => (
              <button
                key={a}
                onClick={() => setAgentAttention(a)}
                className={cn(
                  'rounded border px-2 py-1 text-[11px] transition-colors',
                  agentAttention === a
                    ? 'border-primary/60 bg-primary/15 text-foreground'
                    : 'border-border text-muted-foreground hover:bg-accent/40',
                )}
              >
                {a}
              </button>
            ))}
            <span className="text-[10px] text-muted-foreground">
              drives the real store setter — the ACP wiring lands in a later slice
            </span>
          </div>
          <div className="relative h-[480px] overflow-hidden rounded-lg border border-border bg-gradient-to-b from-[#0a1420] to-[#050a12]">
            <div className="absolute inset-0 flex items-center justify-center text-[11px] text-muted-foreground/40">
              (viewport stand-in)
            </div>
            <Blackboard />
          </div>
        </Section>

        <Section
          title="Streaming — math and cards buffer to completeness"
          note="Prose streams fast; an expression or typed card is withheld behind the chalk cursor and typeset exactly once when its closing delimiter arrives. Settled chunks never re-render."
        >
          <StreamingDemo />
        </Section>

        <Section
          title="Typed cards"
          note="Each fixture is the literal line text an agent would write — a roshera:* fence — rendered through MessageMarkdown, so the fence-to-card path is what you are looking at. Cards arrive with the chalk-draw reveal: click anywhere (or any key) to complete it instantly."
        >
          <button
            onClick={() => setReplayKey((k) => k + 1)}
            className="cad-icon-btn mb-2 h-6 px-1.5 text-[11px]"
            title="Re-mount the cards to replay the reveal"
          >
            <RotateCcw size={11} /> <span className="ml-1">replay reveals</span>
          </button>
          <RevealContext.Provider value={{ animate: true }}>
            <div key={replayKey} className="space-y-5">
              {CARD_FIXTURES.map((f) => (
                <div key={f.title}>
                  <div className="mb-1 text-xs font-medium text-foreground/90">{f.title}</div>
                  <div className="mb-1 max-w-3xl text-[10px] text-muted-foreground">{f.note}</div>
                  <div className="text-sm">
                    <MessageMarkdown content={f.source} />
                  </div>
                </div>
              ))}
            </div>
          </RevealContext.Provider>
        </Section>

        <Section
          title="GD&T symbol reference"
          note="The full notation set from lib/gdt-symbols.ts (code points verified against the Unicode chart nameslists; coverage verified in Segoe UI Symbol). If any glyph below renders as a placeholder box, the font fallback is broken — report it rather than shipping a lookalike."
        >
          <SymbolReference />
        </Section>
      </div>
    </div>
  )
}
