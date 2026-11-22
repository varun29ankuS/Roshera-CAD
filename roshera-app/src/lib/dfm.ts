/**
 * Design for manufacturing: run the kernel's DFM rule packs on a part and
 * read the report in an engineer's words.
 *
 * The report is the kernel's own (`POST /api/agent/parts/{id}/dfm`,
 * `geometry_engine::dfm::DfmReport`). Every rule answers one of three
 * ways: a proven pass, a proven violation naming its faces, or "could not
 * be checked" with the reason. The last is never shown as a pass -- a
 * check that did not run is not a check that passed.
 */

import type { DfmAnimation } from './dfm-animation'

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

export interface DfmValue {
  value: number
  derivation?: { kind: string; method?: string; surface_type?: string }
}

export type DfmVerdict =
  | { kind: 'pass'; margin: DfmValue }
  | { kind: 'violation'; witnesses: number[]; measured: DfmValue; limit: DfmValue }
  | { kind: 'unverifiable'; regions: number[]; reason: { kind: string; detail?: string } }

export interface DfmRuleVerdict {
  rule: string
  verdict: DfmVerdict
  provenance: { kind: string; note?: string; [k: string]: unknown }
}

type V3 = [number, number, number]

/** A pack's request body. The process packs take every threshold as an
 *  optional field: what the caller leaves out takes the kernel's cited
 *  default, and the report echoes the values actually used. */
export type DfmPackParams =
  | { pack: 'fdm'; nozzle_diameter: number; build_direction: V3 }
  | { pack: 'injection_molding'; pull_direction: V3; min_draft_deg: number }
  | { pack: 'cnc_3_axis'; setup_directions?: V3[]; min_tool_diameter?: number; [k: string]: unknown }
  | { pack: 'cnc_turning'; spindle_axis?: [V3, V3] | null; [k: string]: unknown }
  | { pack: 'sheet_metal'; thickness?: number | null; [k: string]: unknown }
  | { pack: 'casting'; pull_direction?: V3; min_draft_deg?: number; [k: string]: unknown }

export interface DfmReport {
  pack: string
  params: DfmPackParams
  verdicts: DfmRuleVerdict[]
  summary: { kind: 'pass' } | { kind: 'violations'; count: number } | { kind: 'inconclusive'; unverifiable: number }
  /** One explanation per violation, built by the kernel from the same
   *  measured values (see `dfm-animation.ts`). */
  animations?: DfmAnimation[]
}

export type DfmState = 'fix' | 'unknown' | 'pass'

/** The processes whose rules exist in the kernel. */
export const DFM_PROCESSES = [
  { pack: 'cnc_3_axis', label: 'CNC milling (3-axis)' },
  { pack: 'cnc_turning', label: 'CNC turning' },
  { pack: 'sheet_metal', label: 'Sheet metal' },
  { pack: 'casting', label: 'Die casting' },
  { pack: 'injection_molding', label: 'Injection moulding' },
  { pack: 'fdm', label: '3D printing (FDM)' },
] as const

/** How each rule reads: its name, its unit, and whether the limit is a
 *  ceiling (≤) or a floor (≥). */
const RULES: Record<string, { title: string; unit: string; cmp: '≤' | '≥' }> = {
  'fdm.overhang': { title: 'Overhang', unit: '°', cmp: '≤' },
  'fdm.min_wall': { title: 'Wall thickness', unit: ' mm', cmp: '≥' },
  'fdm.min_bore': { title: 'Hole size', unit: ' mm', cmp: '≥' },
  'fdm.trapped_volume': { title: 'Trapped volume', unit: '', cmp: '≤' },
  'mold.draft': { title: 'Draft', unit: '°', cmp: '≥' },
  'mold.undercut': { title: 'Undercut', unit: '', cmp: '≤' },
  'mold.trapped_core': { title: 'Trapped core', unit: '', cmp: '≤' },
  'mold.side_action': { title: 'Hole needs a side action', unit: '°', cmp: '≤' },
  'mold.sharp_internal_corner': { title: 'Sharp inside corners', unit: '', cmp: '≤' },
  'cnc.internal_corner_radius': { title: 'Inside corner radius', unit: ' mm', cmp: '≥' },
  'cnc.corner_reach': { title: 'Corner tool reach (L/D)', unit: '', cmp: '≤' },
  'cnc.hole_depth_ratio': { title: 'Hole depth (L/D)', unit: '', cmp: '≤' },
  'cnc.min_hole_diameter': { title: 'Smallest hole', unit: ' mm', cmp: '≥' },
  'cnc.standard_drill_size': { title: 'Standard drill size', unit: ' mm off', cmp: '≤' },
  'cnc.blind_hole_floor': { title: 'Blind hole floor', unit: '° off', cmp: '≤' },
  'cnc.hole_access': { title: 'Hole reachable from a setup', unit: '°', cmp: '≤' },
  'cnc.min_wall': { title: 'Wall thickness', unit: ' mm', cmp: '≥' },
  'turn.axisymmetry': { title: 'Turnable faces', unit: ' off-axis', cmp: '≤' },
  'turn.bore_depth_ratio': { title: 'Bore depth (L/D)', unit: '', cmp: '≤' },
  'turn.min_wall': { title: 'Wall thickness', unit: ' mm', cmp: '≥' },
  'sheet.uniform_thickness': { title: 'Uniform thickness', unit: ' mm', cmp: '≥' },
  'sheet.bend_radius': { title: 'Bend radius', unit: ' mm', cmp: '≥' },
  'sheet.hole_to_edge': { title: 'Hole to edge', unit: ' mm', cmp: '≥' },
  'sheet.hole_to_bend': { title: 'Hole to bend', unit: ' mm', cmp: '≥' },
  'sheet.flange_length': { title: 'Flange length', unit: ' mm', cmp: '≥' },
  'sheet.developable': { title: 'Unfoldable faces', unit: ' doubly curved', cmp: '≤' },
  'cast.draft': { title: 'Draft', unit: '°', cmp: '≥' },
  'cast.min_wall': { title: 'Thinnest wall', unit: ' mm', cmp: '≥' },
  'cast.max_wall': { title: 'Thickest wall', unit: ' mm', cmp: '≤' },
  'cast.internal_fillet': { title: 'Inside fillets', unit: ' mm', cmp: '≥' },
  'cast.cored_hole': { title: 'Cored hole size', unit: ' mm', cmp: '≥' },
  'cast.side_core': { title: 'Hole needs a side core', unit: '°', cmp: '≤' },
  'cast.trapped_core': { title: 'Trapped core', unit: '', cmp: '≤' },
}

function ruleInfo(rule: string) {
  const known = RULES[rule]
  if (known) return known
  const tail = rule.split('.').pop() ?? rule
  const title = tail.replace(/_/g, ' ')
  return { title: title.charAt(0).toUpperCase() + title.slice(1), unit: '', cmp: '≤' as const }
}

const num = (v: number) => (Number.isInteger(v) ? String(v) : String(Math.round(v * 100) / 100))

export interface DfmRow {
  rule: string
  title: string
  state: DfmState
  /** The measured value (violations), or the headroom (passes). */
  measured: string
  /** The limit, with its direction, e.g. "≤ 45°". Empty when not a violation. */
  limit: string
  /** Why a check could not run (unknown rows). */
  why: string
  /** The faces the finding is about. */
  faces: number[]
  /** Where the rule's threshold comes from. */
  source: string
}

const ORDER: Record<DfmState, number> = { fix: 0, unknown: 1, pass: 2 }

/** One row per rule, the ones to fix first, then the ones that could not
 *  be checked, then the passes. */
export function dfmRows(report: DfmReport): DfmRow[] {
  const rows = report.verdicts.map((rv): DfmRow => {
    const info = ruleInfo(rv.rule)
    const source = rv.provenance.note ?? rv.provenance.kind.replace(/_/g, ' ')
    const v = rv.verdict
    if (v.kind === 'violation') {
      return {
        rule: rv.rule, title: info.title, state: 'fix',
        measured: `${num(v.measured.value)}${info.unit}`,
        limit: `${info.cmp} ${num(v.limit.value)}${info.unit}`,
        why: '', faces: [...v.witnesses], source,
      }
    }
    if (v.kind === 'unverifiable') {
      return {
        rule: rv.rule, title: info.title, state: 'unknown', measured: '', limit: '',
        why: v.reason.detail ?? v.reason.kind.replace(/_/g, ' '),
        faces: [...v.regions], source,
      }
    }
    return {
      rule: rv.rule, title: info.title, state: 'pass',
      // The kernel's vacuous-pass sentinel (1e300): no feature of the kind
      // on this part — nothing to judge, not a huge margin.
      measured:
        v.margin.value >= 1e299 ? 'none on this part' : info.unit ? `${num(v.margin.value)}${info.unit} to spare` : '',
      limit: '', why: '', faces: [], source,
    }
  })
  return rows.sort((a, b) => ORDER[a.state] - ORDER[b.state])
}

/** Faces to colour: red where a rule is proven broken, amber where a check
 *  could not run. A face that is red for one rule stays red. */
export function dfmFaceTints(report: DfmReport): { fix: number[]; unknown: number[] } {
  const fix = new Set<number>()
  const unknown = new Set<number>()
  for (const rv of report.verdicts) {
    if (rv.verdict.kind === 'violation') rv.verdict.witnesses.forEach((f) => fix.add(f))
    if (rv.verdict.kind === 'unverifiable') rv.verdict.regions.forEach((f) => unknown.add(f))
  }
  for (const f of fix) unknown.delete(f)
  const sorted = (s: Set<number>) => [...s].sort((a, b) => a - b)
  return { fix: sorted(fix), unknown: sorted(unknown) }
}

/** The one-line verdict for the whole part. */
export function dfmSummary(summary: DfmReport['summary']): { state: DfmState; text: string } {
  if (summary.kind === 'violations') return { state: 'fix', text: `${summary.count} to fix` }
  if (summary.kind === 'inconclusive') {
    const n = summary.unverifiable
    return { state: 'unknown', text: `${n} check${n === 1 ? '' : 's'} could not run` }
  }
  return { state: 'pass', text: 'Every check passed' }
}

export type DfmRun =
  | { kind: 'report'; report: DfmReport }
  /** The part changed since it was last verified; the kernel will not judge
   *  unverified geometry. Verify it, then run again. */
  | { kind: 'unverified'; reason: string }
  | { kind: 'refused'; reason: string }

/** Run one process's rules on a part (by its kernel solid id). */
export async function runDfm(solidId: number, params: DfmPackParams): Promise<DfmRun> {
  const r = await fetch(`${API_HOST}/api/agent/parts/${solidId}/dfm`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(params),
  })
  const body = (await r.json().catch(() => ({}))) as Record<string, unknown>
  if (r.ok) return { kind: 'report', report: body as unknown as DfmReport }
  const reason = typeof body.reason === 'string' ? body.reason : `HTTP ${r.status}`
  if (body.error === 'dfm_stale_solid') return { kind: 'unverified', reason }
  return { kind: 'refused', reason }
}

/** Verify a part (the kernel's full certificate), which DFM needs first. */
export async function verifyPart(solidId: number): Promise<boolean> {
  const r = await fetch(`${API_HOST}/api/agent/parts/${solidId}/perception`)
  return r.ok
}
