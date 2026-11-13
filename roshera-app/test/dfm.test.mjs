import { test } from 'node:test'
import assert from 'node:assert/strict'

const { dfmRows, dfmFaceTints, dfmSummary } = await import('../src/lib/dfm.ts')

// Trimmed from the real FDM report on the hub flange (solid 12), 2026-11-13.
const fdm = {
  pack: 'fdm',
  params: { pack: 'fdm', nozzle_diameter: 0.4, build_direction: [0, 0, 1] },
  verdicts: [
    { rule: 'fdm.trapped_volume', verdict: { kind: 'pass', margin: { value: 0, derivation: { kind: 'analytic' } } }, provenance: { kind: 'shop_practice', note: 'no enclosed voids' } },
    { rule: 'fdm.min_wall', verdict: { kind: 'unverifiable', regions: [56, 57, 62], reason: { kind: 'unsupported_topology', detail: 'candidate wall pair (faces 56, 57): loop shape does not admit an exact overlap in v1' } }, provenance: { kind: 'shop_practice', note: 'wall thickness >= 2x nozzle diameter' } },
    { rule: 'fdm.overhang', verdict: { kind: 'violation', witnesses: [56], measured: { value: 90, derivation: { kind: 'analytic' } }, limit: { value: 45, derivation: { kind: 'analytic' } } }, provenance: { kind: 'shop_practice', note: '45° overhang; practice-derived, no governing standard' } },
  ],
  summary: { kind: 'violations', count: 1 },
}

test('findings to fix come first, then the ones that could not be checked, then passes', () => {
  assert.deepEqual(dfmRows(fdm).map((r) => [r.rule, r.state]), [
    ['fdm.overhang', 'fix'],
    ['fdm.min_wall', 'unknown'],
    ['fdm.trapped_volume', 'pass'],
  ])
})

test('a violation reads as measured against the limit, in the rule’s unit', () => {
  const r = dfmRows(fdm)[0]
  assert.equal(r.title, 'Overhang')
  assert.equal(r.measured, '90°')
  assert.equal(r.limit, '≤ 45°')
  assert.deepEqual(r.faces, [56])
  assert.match(r.source, /practice-derived/)
})

test('a check that could not run says why, and is never a pass', () => {
  const r = dfmRows(fdm)[1]
  assert.equal(r.title, 'Wall thickness')
  assert.equal(r.state, 'unknown')
  assert.match(r.why, /exact overlap/)
  assert.deepEqual(r.faces, [56, 57, 62])
})

test('faces to fix are red, faces that could not be checked are amber, a red face stays red', () => {
  assert.deepEqual(dfmFaceTints(fdm), { fix: [56], unknown: [57, 62] })
})

test('the summary never calls an unchecked part manufacturable', () => {
  assert.deepEqual(dfmSummary({ kind: 'violations', count: 1 }), { state: 'fix', text: '1 to fix' })
  assert.deepEqual(dfmSummary({ kind: 'inconclusive', unverifiable: 2 }), { state: 'unknown', text: '2 checks could not run' })
  assert.deepEqual(dfmSummary({ kind: 'pass' }), { state: 'pass', text: 'Every check passed' })
})

test('draft from the moulding pack reads in degrees with its minimum', () => {
  const mold = {
    pack: 'injection_molding',
    params: { pack: 'injection_molding', pull_direction: [0, 0, 1], min_draft_deg: 1 },
    verdicts: [{ rule: 'mold.draft', verdict: { kind: 'violation', witnesses: [63, 64, 65], measured: { value: 0, derivation: { kind: 'analytic' } }, limit: { value: 1, derivation: { kind: 'analytic' } } }, provenance: { kind: 'shop_practice', note: 'draft angle' } }],
    summary: { kind: 'violations', count: 1 },
  }
  const r = dfmRows(mold)[0]
  assert.deepEqual([r.title, r.measured, r.limit], ['Draft', '0°', '≥ 1°'])
})
