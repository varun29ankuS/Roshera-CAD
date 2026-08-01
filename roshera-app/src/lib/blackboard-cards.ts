/**
 * TYPED BLACKBOARD CARDS
 * ======================
 * The agent's most valuable outputs are structured — a soundness certificate,
 * a DFM rule verdict, a GD&T feature control frame with its certified verdict,
 * a typed refusal, a merge result with conflict witnesses. Flattened into
 * prose they lose exactly what makes them verifiable, so a Blackboard line can
 * embed them as a fenced block the renderer turns into a typed card:
 *
 * ```roshera:dfm
 * { "rule": "fdm.min_wall", "verdict": { ... }, "provenance": { ... } }
 * ```
 *
 * The fence convention keeps cards inside the editable-line model: the raw
 * JSON is the line's source (visible and editable in the textarea), the card
 * is its rendering — the same relationship markdown already has. A payload
 * that fails validation renders as the raw fence, never a half-card: an
 * honest fallback over a fabricated rendering.
 *
 * # Payload provenance — read from the real wire shapes, not invented
 * - `roshera:dfm`      → `RuleVerdict` verbatim (geometry-engine/src/dfm/
 *   report.rs: `Verdict`, `DfmValue`, `Derivation`, `DfmBound`,
 *   `UnverifiableReason`; provenance.rs: `RuleProvenance`). serde tagging is
 *   `#[serde(tag = "kind", rename_all = "snake_case")]` throughout.
 * - `roshera:fcf`      → the gdt verdict fields the MCP layer reads
 *   (roshera-mcp/src/tools/gdt.ts: `characteristic`, `tolerance_mm`,
 *   `tolerance_label`, `datum_statuses[].label`, `conforms`, `measured_mm`,
 *   `measured_label`, `fit_residual_mm`, `reason`, `frame.origin`,
 *   `frame.derivation`), wrapped in an authoring envelope that can also
 *   express what the kernel schema deliberately CANNOT (modifiers, the
 *   non-certified characteristics) — which is precisely what makes such a
 *   frame design intent rather than a certificate.
 * - `roshera:merge`    → `MergeView` verbatim (api-server/src/branches.rs:
 *   `ConflictView`, `ConflictWitnessView`, `MergeStatisticsView`).
 * - `roshera:soundness`→ the perception/certificate projection the MCP layer
 *   already builds (roshera-mcp/src/core.ts: `sound`, `brep_valid`,
 *   `watertight`, `manifold`, `self_intersection_free`,
 *   `construction_consistent`, `labels_consistent`, `tessellation_clean`,
 *   `mesh_quality_clean`, `euler_characteristic`, `eyes_consistent`,
 *   `open_edges`, `nonmanifold_edges`, `face_count`, `volume`, `errors`) —
 *   tri-state per invariant: true / false / null-means-not-run.
 * - `roshera:refusal`  → the refusal wire the backend already speaks: a
 *   verbatim `message` from a 409/422 (never paraphrased — gdt.ts surfaces
 *   it as `REFUSED: <msg>`), plus optional next actions. A refusal is a
 *   RESULT, not an error.
 * - `roshera:choices`  → NOT a kernel wire type — an authoring convention
 *   (`.goosehints`, "When you need the human to choose between discrete
 *   options") the agent uses to ask a genuinely closed-set question as
 *   clickable buttons instead of prose the user has to retype. YAML, not
 *   JSON (the block is hand-authored by the agent, and YAML is what
 *   `.goosehints` specifies). `selected` is added by the UI, never by the
 *   agent, once the user has answered — it makes the board a record rather
 *   than a still-open question. There is deliberately NO prose-parsing
 *   fallback: an invalid block renders as raw text, never guessed buttons.
 *
 * Envelope fields beyond the wire payloads (`unit`, `note`, `standard`,
 * `options`, `part`) are authoring annotations — presentation the agent adds,
 * clearly separated from what the kernel asserted.
 */

import { z } from 'zod'
import { parse as parseYaml } from 'yaml'

// ── DFM (geometry-engine/src/dfm/report.rs) ───────────────────────────

const dfmDerivationSchema = z.discriminatedUnion('kind', [
  z
    .object({
      kind: z.literal('analytic'),
      surface_type: z.string(),
      method: z.string(),
    })
    .passthrough(),
  z
    .object({
      kind: z.literal('bounded_analytic'),
      method: z.string(),
      refinement_depth: z.number().int().nonnegative(),
      converged: z.boolean(),
    })
    .passthrough(),
])

const dfmBoundSchema = z.object({ lo: z.number(), hi: z.number() })

const dfmValueSchema = z.object({
  value: z.number(),
  derivation: dfmDerivationSchema,
  bound: dfmBoundSchema.optional(),
})

const unverifiableReasonSchema = z.discriminatedUnion('kind', [
  z
    .object({
      kind: z.literal('unsupported_surface'),
      surface_type: z.string(),
      analyzer: z.string(),
    })
    .passthrough(),
  z.object({ kind: z.literal('unsound_precondition'), detail: z.string() }).passthrough(),
  z.object({ kind: z.literal('unsupported_topology'), detail: z.string() }).passthrough(),
  z
    .object({
      kind: z.literal('bound_not_separating'),
      lo: z.number(),
      hi: z.number(),
      limit: z.number(),
      refinement_depth: z.number().int().nonnegative(),
      converged: z.boolean(),
    })
    .passthrough(),
])

const dfmVerdictSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('pass'), margin: dfmValueSchema }).passthrough(),
  z
    .object({
      kind: z.literal('violation'),
      witnesses: z.array(z.number().int()),
      measured: dfmValueSchema,
      limit: dfmValueSchema,
    })
    .passthrough(),
  z
    .object({
      kind: z.literal('unverifiable'),
      regions: z.array(z.number().int()),
      reason: unverifiableReasonSchema,
    })
    .passthrough(),
])

const ruleProvenanceSchema = z.discriminatedUnion('kind', [
  z
    .object({
      kind: z.literal('standard'),
      body: z.string(),
      designation: z.string(),
      edition: z.string(),
      clause: z.string().optional(),
    })
    .passthrough(),
  z.object({ kind: z.literal('handbook'), citation: z.string() }).passthrough(),
  z.object({ kind: z.literal('material_datasheet'), source: z.string() }).passthrough(),
  z.object({ kind: z.literal('shop_practice'), note: z.string() }).passthrough(),
])

/** `RuleVerdict` verbatim + authoring annotations (`unit`, `note`). */
export const dfmCardSchema = z.object({
  rule: z.string(),
  verdict: dfmVerdictSchema,
  provenance: ruleProvenanceSchema,
  /** Authoring annotation: display unit for the rule's values ("mm", "°").
   *  NOT a kernel field — the pack defines each rule's domain. */
  unit: z.string().optional(),
  note: z.string().optional(),
})

export type DfmCard = z.infer<typeof dfmCardSchema>
export type DfmCardVerdict = z.infer<typeof dfmVerdictSchema>
export type DfmCardValue = z.infer<typeof dfmValueSchema>
export type DfmUnverifiableReason = z.infer<typeof unverifiableReasonSchema>
export type DfmRuleProvenance = z.infer<typeof ruleProvenanceSchema>

// ── GD&T FCF (roshera-mcp/src/tools/gdt.ts wire fields) ───────────────

const gdtDatumStatusSchema = z
  .object({
    label: z.string(),
    // gdt.ts `compactDatum` reads resolution.status: 'live' | 'dangling'
    status: z.string().optional(),
  })
  .passthrough()

const gdtFrameSchema = z
  .object({
    origin: z.array(z.number()).min(2),
    derivation: z.string().optional(),
  })
  .passthrough()

/** The kernel verdict fields, verbatim from the gdt wire. */
const gdtVerdictSchema = z
  .object({
    conforms: z.enum(['in_spec', 'out_of_spec', 'not_evaluable']),
    tolerance_mm: z.number().optional(),
    tolerance_label: z.string().optional(),
    measured_mm: z.number().optional(),
    measured_label: z.string().optional(),
    fit_residual_mm: z.number().optional(),
    reason: z.string().optional(),
    datum_statuses: z.array(gdtDatumStatusSchema).optional(),
    frame: gdtFrameSchema.optional(),
  })
  .passthrough()

export const fcfCardSchema = z.object({
  /** Canonical characteristic id — the four certified ones, or any of the
   *  full notation set (straightness, cylindricity, total_runout, …). */
  characteristic: z.string(),
  tolerance: z.object({
    value_mm: z.number().optional(),
    /** Preformatted label (e.g. "0.10 mm") when value_mm alone is not it. */
    label: z.string().optional(),
    /** ⌀ — cylindrical zone. The one modifier the kernel's position
     *  evaluation already embodies. */
    diameter: z.boolean().optional(),
    /** S⌀ — spherical zone (composed notation, uncertified). */
    spherical_diameter: z.boolean().optional(),
    /** Material-condition / zone modifier id (mmc, lmc, projected, …).
     *  The kernel schema has NO such field — any value here makes the
     *  frame design intent, not a certificate. */
    modifier: z.string().optional(),
  }),
  /** Ordered datum references as authored (A, B, C). The kernel echo, when
   *  present, is `verdict.datum_statuses`. */
  datums: z.array(z.string()).optional(),
  /** Governing dialect, stated once per the policy doc. Decides how the
   *  Y14.5-2018 removals (concentricity, symmetry) are flagged. */
  standard: z.enum(['asme-y14.5-2018', 'iso-gps']).optional(),
  /** Kernel verdict, verbatim, when the frame was actually evaluated.
   *  Absent → the frame is authored intent awaiting (or outside) kernel
   *  evaluation. */
  verdict: gdtVerdictSchema.optional(),
  note: z.string().optional(),
})

export type FcfCard = z.infer<typeof fcfCardSchema>
export type FcfVerdict = z.infer<typeof gdtVerdictSchema>

// ── Refusal ───────────────────────────────────────────────────────────

export const refusalCardSchema = z.object({
  /** The backend's message VERBATIM — never paraphrased (the gdt.ts rule). */
  reason: z.string(),
  /** What was refused, named by role ("datum B on the upper bore", …). */
  subject: z.string().optional(),
  /** Where the refusal came from ("kernel", "api", a tool name). */
  source: z.string().optional(),
  /** Next actions — a refusal is a result with options, not a dead end. */
  options: z.array(z.string()).optional(),
})

export type RefusalCard = z.infer<typeof refusalCardSchema>

// ── Choices (authoring convention — see the top-of-file note) ─────────

const choiceOptionSchema = z.object({
  /** Sent verbatim as the next turn when this option is clicked. */
  value: z.string().min(1),
  /** Button text. */
  label: z.string().min(1),
  /** Secondary text beneath the label. */
  detail: z.string().optional(),
})

export const choicesCardSchema = z
  .object({
    question: z.string().min(1),
    options: z.array(choiceOptionSchema).min(1),
    /** UI-authored, never the agent: the value the user clicked. Absent
     *  means the question is still open. */
    selected: z.string().optional(),
  })
  // A `selected` that names no real option would be a fabricated answer —
  // refuse the render (falls back to raw text) rather than show it anyway.
  .refine((c) => c.selected === undefined || c.options.some((o) => o.value === c.selected), {
    message: 'selected does not match any option value',
    path: ['selected'],
  })

export type ChoicesCard = z.infer<typeof choicesCardSchema>
export type ChoiceOption = z.infer<typeof choiceOptionSchema>

// ── Detected (unfenced) choices ────────────────────────────────────────
//
// `.goosehints` is steering — an instruction the agent can ignore — so the
// board also enforces the outcome as a constraint: an agent line that asks a
// closed question as "Option A: … Option B: …" prose, without the
// `roshera:choices` fence, still gets clickable buttons. Deliberately
// narrow: this must never INVENT an option, only recognise one the agent
// itself explicitly labelled.

export interface DetectedChoiceOption {
  /** The label as written — "A", "B", "1" — display-only. */
  label: string
  /** Everything after "Option X:" on that line, the agent's own words,
   *  verbatim. This is what gets sent as the reply when clicked. */
  text: string
}

export interface DetectedChoiceSet {
  options: DetectedChoiceOption[]
}

/** "Option A:", "Option B:", "Option 1:" … at the start of a (trimmed)
 *  line. Deliberately just letters/digits — no bullets, no "Option A -",
 *  nothing inferred beyond what the agent explicitly labelled. */
const OPTION_LINE_RE = /^Option\s+([A-Za-z0-9]+):\s*(.+)$/

/**
 * Scan one agent-authored line's full text for its own "Option A: …" /
 * "Option B: …" enumeration. Requires at least two matching lines within
 * the same text to count as a genuine closed set — a single "Option A:" is
 * not a choice. If the text already contains a `roshera:choices` fence,
 * that path already renders buttons, so this defers unconditionally
 * (returns `null`) rather than double-rendering.
 *
 * Callers MUST gate this to `line.author === 'agent'` themselves — this
 * function has no notion of authorship, only text.
 */
export function detectEnumeratedChoices(text: string): DetectedChoiceSet | null {
  if (text.includes('```roshera:choices')) return null
  const options: DetectedChoiceOption[] = []
  for (const rawLine of text.split('\n')) {
    const match = OPTION_LINE_RE.exec(rawLine.trim())
    if (match) options.push({ label: match[1], text: match[2].trim() })
  }
  return options.length >= 2 ? { options } : null
}

// ── Merge result (api-server/src/branches.rs `MergeView`) ─────────────

const conflictWitnessSchema = z
  .object({
    id: z.string(),
    sequence_number: z.number().int().nonnegative(),
    timestamp: z.string(),
    operation_type: z.string(),
    author: z.string(),
    /** Full structured operation as tagged JSON — inspected, not rendered. */
    operation: z.unknown().optional(),
  })
  .passthrough()

const conflictViewSchema = z
  .object({
    subject: z.string(),
    /** Taxonomy verdict: concurrent_modification | delete_modify |
     *  operation_conflict | dependency_conflict | topological_conflict. */
    conflict_type: z.string(),
    source_event: conflictWitnessSchema.nullish(),
    target_event: conflictWitnessSchema.nullish(),
    /** Derived from the typed fields, never the other way around. */
    summary: z.string(),
  })
  .passthrough()

export const mergeCardSchema = z.object({
  success: z.boolean(),
  merged_into: z.string(),
  strategy: z.string().optional(),
  events_merged: z.number().int().nonnegative().optional(),
  conflicts: z.array(conflictViewSchema).default([]),
  statistics: z
    .object({
      events_merged: z.number().int().nonnegative(),
      conflicts_count: z.number().int().nonnegative(),
      auto_resolved: z.number().int().nonnegative(),
      entities_affected: z.number().int().nonnegative(),
      duration_ms: z.number().nonnegative(),
    })
    .passthrough()
    .optional(),
  /** Authoring annotation: the source branch's display name. */
  source: z.string().optional(),
})

export type MergeCard = z.infer<typeof mergeCardSchema>
export type MergeConflict = z.infer<typeof conflictViewSchema>
export type MergeConflictWitness = z.infer<typeof conflictWitnessSchema>

// ── Soundness certificate (roshera-mcp/src/core.ts projection) ────────

/** true / false / null — null means NOT RUN, and not-run ≠ passed. */
const triState = z.boolean().nullish()

export const soundnessCardSchema = z.object({
  /** Authoring annotation: the part's display name. */
  part: z.string().optional(),
  sound: triState,
  brep_valid: triState,
  watertight: triState,
  manifold: triState,
  self_intersection_free: triState,
  construction_consistent: triState,
  labels_consistent: triState,
  tessellation_clean: triState,
  mesh_quality_clean: triState,
  eyes_consistent: triState,
  euler_characteristic: z.number().nullish(),
  open_edges: z.number().nullish(),
  nonmanifold_edges: z.number().nullish(),
  face_count: z.number().nullish(),
  volume: z.number().nullish(),
  errors: z.array(z.string()).nullish(),
})

export type SoundnessCard = z.infer<typeof soundnessCardSchema>

// ── Fence parsing ─────────────────────────────────────────────────────

export const CARD_FENCE_PREFIX = 'roshera:'

export type BlackboardCard =
  | { kind: 'dfm'; card: DfmCard }
  | { kind: 'fcf'; card: FcfCard }
  | { kind: 'refusal'; card: RefusalCard }
  | { kind: 'merge'; card: MergeCard }
  | { kind: 'soundness'; card: SoundnessCard }
  | { kind: 'choices'; card: ChoicesCard }

export type CardKind = BlackboardCard['kind']

const CARD_KINDS: readonly CardKind[] = ['dfm', 'fcf', 'refusal', 'merge', 'soundness', 'choices']

/** `choices` is hand-authored YAML per `.goosehints`; every other card is a
 *  JSON echo of a wire type. Malformed input in either format yields `null`
 *  here — the caller reports it as a parse error, never a half-card. */
function parseCardSource(kind: CardKind, source: string): { ok: true; json: unknown } | { ok: false; error: string } {
  try {
    return { ok: true, json: kind === 'choices' ? parseYaml(source) : JSON.parse(source) }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : `invalid ${kind === 'choices' ? 'YAML' : 'JSON'}` }
  }
}

/** Extract a card kind from a fence language / markdown code className
 *  (`language-roshera:dfm` or bare `roshera:dfm`). */
export function cardKindFromLanguage(language: string | undefined): CardKind | null {
  if (!language) return null
  const lang = language.replace(/^language-/, '').trim().toLowerCase()
  if (!lang.startsWith(CARD_FENCE_PREFIX)) return null
  const kind = lang.slice(CARD_FENCE_PREFIX.length)
  return (CARD_KINDS as readonly string[]).includes(kind) ? (kind as CardKind) : null
}

export type CardParseResult =
  | { ok: true; card: BlackboardCard }
  | { ok: false; error: string }

/** Parse + validate a fenced card payload. Invalid JSON or a shape the
 *  schema rejects yields a typed error — the caller falls back to rendering
 *  the raw fence (honest, never a half-card). */
export function parseCard(kind: CardKind, source: string): CardParseResult {
  const parsed = parseCardSource(kind, source)
  if (!parsed.ok) return { ok: false, error: parsed.error }
  const json = parsed.json
  const fail = (r: z.SafeParseReturnType<unknown, unknown>): CardParseResult => ({
    ok: false,
    error:
      r.success || r.error.issues.length === 0
        ? 'schema mismatch'
        : r.error.issues
            .slice(0, 3)
            .map((i) => `${i.path.join('.') || '(root)'}: ${i.message}`)
            .join('; '),
  })
  switch (kind) {
    case 'dfm': {
      const r = dfmCardSchema.safeParse(json)
      return r.success ? { ok: true, card: { kind, card: r.data } } : fail(r)
    }
    case 'fcf': {
      const r = fcfCardSchema.safeParse(json)
      return r.success ? { ok: true, card: { kind, card: r.data } } : fail(r)
    }
    case 'refusal': {
      const r = refusalCardSchema.safeParse(json)
      return r.success ? { ok: true, card: { kind, card: r.data } } : fail(r)
    }
    case 'merge': {
      const r = mergeCardSchema.safeParse(json)
      return r.success ? { ok: true, card: { kind, card: r.data } } : fail(r)
    }
    case 'soundness': {
      const r = soundnessCardSchema.safeParse(json)
      return r.success ? { ok: true, card: { kind, card: r.data } } : fail(r)
    }
    case 'choices': {
      const r = choicesCardSchema.safeParse(json)
      return r.success ? { ok: true, card: { kind, card: r.data } } : fail(r)
    }
  }
}
