import { test } from 'node:test'
import assert from 'node:assert/strict'

const { parseCard, invariantState, isDfmReport } = await import('../src/lib/blackboard-cards.ts')

// verify_part output, verbatim from a live Blackboard line (2026-11-15).
const soundness = {"part_id":12,"sound":true,"brep_valid":true,"brep_watertight":true,"manifold":true,"shells_outward":true,"misoriented_shells":[],"self_intersection_free":true,"tessellation_clean":true,"mesh_quality_clean":true,"construction_consistent":"not_applicable","eyes_consistent":"consistent","verdict":"SOUND — full kernel certificate clean (closed, manifold, self-intersection-free, mesh-quality-clean)","cert":{"sound":true,"brep_valid":true,"watertight":true,"manifold":true,"oriented":true,"self_intersection_free":true,"euler_characteristic":-8,"boundary_edges":0,"nonmanifold_edges":0,"inconsistent_directed_edges":0,"errors":[],"model_debris_orphan_faces":0}}

// dfm_check output (the kernel's DfmReport), verbatim from the same session.
const dfmReport = {"pack":"fdm","params":{"pack":"fdm","nozzle_diameter":0.4,"build_direction":[0,0,1]},"verdicts":[{"rule":"fdm.overhang","verdict":{"kind":"unverifiable","regions":[58,59,60,61,63,64,65],"reason":{"kind":"unsupported_surface","surface_type":"cylinder","analyzer":"face_orientation_field (...)"}},"provenance":{"kind":"shop_practice","note":"45° overhang; practice-derived, no governing standard"}},{"rule":"fdm.min_wall","verdict":{"kind":"unverifiable","regions":[56,57,62,63,64,65],"reason":{"kind":"unsupported_topology","detail":"candidate wall pair ..."}},"provenance":{"kind":"shop_practice","note":"wall thickness >= 2x nozzle diameter; ..."}},{"rule":"fdm.trapped_volume","verdict":{"kind":"pass","margin":{"value":0,"derivation":{"kind":"analytic","surface_type":"plane","method":"fdm.trapped_volume: no enclosed voids found"}}},"provenance":{"kind":"shop_practice","note":"..."}}],"summary":{"kind":"inconclusive","unverifiable":3}}

function parsed(kind, payload) {
  const r = parseCard(kind, JSON.stringify(payload))
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  return r.card.card
}

test('the live verify_part payload validates as a soundness card', () => {
  const card = parsed('soundness', soundness)
  assert.equal(card.sound, true)
  assert.equal(card.construction_consistent, 'not_applicable')
  assert.equal(card.eyes_consistent, 'consistent')
})

test('brep_watertight and the nested cert fill the fields the card shows', () => {
  const card = parsed('soundness', soundness)
  assert.equal(card.watertight, true)
  assert.equal(card.euler_characteristic, -8)
  assert.equal(card.open_edges, 0)
  assert.equal(card.nonmanifold_edges, 0)
})

test('tri-state consistency: only inconsistent fails, not_applicable is neither pass nor fail', () => {
  assert.equal(invariantState('consistent'), 'pass')
  assert.equal(invariantState('inconsistent'), 'fail')
  assert.equal(invariantState('not_applicable'), 'na')
  assert.equal(invariantState(true), 'pass')
  assert.equal(invariantState(false), 'fail')
  assert.equal(invariantState(null), 'not_run')
  assert.equal(invariantState(undefined), 'not_run')
})

test('legacy boolean consistency still validates', () => {
  const card = parsed('soundness', { ...soundness, construction_consistent: true, eyes_consistent: null })
  assert.equal(invariantState(card.construction_consistent), 'pass')
  assert.equal(invariantState(card.eyes_consistent), 'not_run')
})

test('an unknown consistency word is refused, not guessed', () => {
  const r = parseCard('soundness', JSON.stringify({ ...soundness, eyes_consistent: 'probably' }))
  assert.equal(r.ok, false)
})

test('the live DfmReport validates as a dfm card', () => {
  const card = parsed('dfm', dfmReport)
  assert.equal(isDfmReport(card), true)
  assert.equal(card.verdicts.length, 3)
  assert.deepEqual(card.summary, { kind: 'inconclusive', unverifiable: 3 })
})

test('unverifiable rules in the report keep their kind, regions and reason', () => {
  const card = parsed('dfm', dfmReport)
  const overhang = card.verdicts[0]
  assert.equal(overhang.verdict.kind, 'unverifiable')
  assert.deepEqual(overhang.verdict.regions, [58, 59, 60, 61, 63, 64, 65])
  assert.equal(overhang.verdict.reason.kind, 'unsupported_surface')
  assert.equal(overhang.provenance.note, '45° overhang; practice-derived, no governing standard')
})

test('a single RuleVerdict still validates as a dfm card', () => {
  const card = parsed('dfm', dfmReport.verdicts[2])
  assert.equal(isDfmReport(card), false)
  assert.equal(card.rule, 'fdm.trapped_volume')
})

test('a report whose summary is missing is refused', () => {
  const { summary: _s, ...rest } = dfmReport
  const r = parseCard('dfm', JSON.stringify(rest))
  assert.equal(r.ok, false)
})
