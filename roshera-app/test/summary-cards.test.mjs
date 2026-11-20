// Run: node --experimental-strip-types --test test/summary-cards.test.mjs
//
// The agent's most common results — "here is the part", "here is the sheet",
// "here are the numbers" — had no card kind, so they arrived as paragraphs.
// Three summary cards, each built from the fields of the real wire shapes:
//   roshera:part     — MassPropertiesReport (geometry-engine readable/part.rs:
//                      volume, surface_area, mass, material, center_of_mass,
//                      provenance) + get_part's location.dimensions_world +
//                      verify_part's `sound`;
//   roshera:sheet    — make_drawing / drawing_read_semantics /
//                      drawing_dimensions (roshera-mcp tools: drawing_id,
//                      name, views, projection, sound, complete, quality,
//                      omission_summary) + the sheet's paper size and scale;
//   roshera:measures — a measured-values table (quantity, value, unit,
//                      limit, source, pass).
// Not-checked is never a tick: `sound` / `complete` / `pass` are tri-state.
import { test } from 'node:test'
import assert from 'node:assert/strict'

const { parseCard, cardKindFromLanguage } = await import('../src/lib/blackboard-cards.ts')
const { lineVerdict } = await import('../src/lib/blackboard-line-state.ts')

const FENCE = '```'
const fenced = (kind, payload) => `${FENCE}roshera:${kind}\n${JSON.stringify(payload)}\n${FENCE}`
const parsed = (kind, payload) => {
  const r = parseCard(kind, JSON.stringify(payload))
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  return r.card.card
}

const PART = {
  part: 'Hub flange',
  part_id: 12,
  dimensions_world: [160, 160, 28],
  volume: 312400.5,
  surface_area: 61230.2,
  mass: 2.452,
  material: { name: 'Steel', density: 7850 },
  center_of_mass: [0, 0, 11.2],
  provenance: {
    volume: { exactness: 'approximate', method: 'mesh_tonon', rel_error_bound: 0.0005 },
    center_of_mass: { exactness: 'exact' },
    inertia: { exactness: 'unavailable' },
  },
  sound: true,
}

test('the fence languages are known card kinds', () => {
  for (const k of ['part', 'sheet', 'measures']) assert.equal(cardKindFromLanguage(`language-roshera:${k}`), k)
})

test('a part summary validates from the mass-properties + location fields', () => {
  const card = parsed('part', PART)
  assert.equal(card.part, 'Hub flange')
  assert.deepEqual(card.dimensions_world, [160, 160, 28])
  assert.equal(card.mass, 2.452)
  assert.equal(card.material.name, 'Steel')
  assert.equal(card.provenance.volume.exactness, 'approximate')
})

test('a part summary with nothing measured is refused, not drawn empty', () => {
  assert.equal(parseCard('part', JSON.stringify({ part: 'Hub flange' })).ok, false)
  assert.equal(parseCard('part', JSON.stringify({ volume: 10 })).ok, false, 'names no part')
  assert.equal(parseCard('part', JSON.stringify({ ...PART, dimensions_world: [1, 2] })).ok, false)
})

test('part verdict: sound ticks, unsound crosses, not-run is no verdict', () => {
  assert.equal(lineVerdict(fenced('part', PART)), 'pass')
  assert.equal(lineVerdict(fenced('part', { ...PART, sound: false })), 'fail')
  assert.equal(lineVerdict(fenced('part', { ...PART, sound: null })), null)
})

const SHEET = {
  name: 'Hub flange',
  drawing_id: '6f1c2b1e-58a4-4f1e-9a51-1f8f2b6f0e11',
  sheet: 'A3',
  scale: '1:2',
  projection: 'first',
  views: ['Front', 'Top', 'Right', 'Iso'],
  dimensions_placed: 18,
  dimensions_omitted: 0,
  sound: true,
  complete: true,
  quality: { passed: true, issues: [] },
}

test('a sheet summary validates from the drawing tool fields', () => {
  const card = parsed('sheet', SHEET)
  assert.equal(card.sheet, 'A3')
  assert.equal(card.projection, 'first')
  assert.deepEqual(card.views, ['Front', 'Top', 'Right', 'Iso'])
  assert.equal(card.quality.passed, true)
})

test('sheet verdict: only sound AND complete AND clean layout is a pass', () => {
  assert.equal(lineVerdict(fenced('sheet', SHEET)), 'pass')
  assert.equal(lineVerdict(fenced('sheet', { ...SHEET, complete: false })), 'fail')
  assert.equal(lineVerdict(fenced('sheet', { ...SHEET, sound: false })), 'fail')
  assert.equal(lineVerdict(fenced('sheet', { ...SHEET, quality: { passed: false, issues: [] } })), 'fail')
  assert.equal(lineVerdict(fenced('sheet', { ...SHEET, complete: null })), 'inconclusive', 'not checked is not a pass')
  const { sound: _s, complete: _c, ...bare } = SHEET
  assert.equal(lineVerdict(fenced('sheet', bare)), 'inconclusive')
})

test('a sheet with an unknown projection is refused', () => {
  assert.equal(parseCard('sheet', JSON.stringify({ ...SHEET, projection: 'second' })).ok, false)
})

const MEASURES = {
  title: 'Bore and wall checks',
  rows: [
    { quantity: 'Bore $\\varnothing$', value: 20.012, unit: 'mm', limit: 'H7: 20.000 to 20.021', source: 'ISO 286-2', pass: true },
    { quantity: 'Wall at the boss', value: 2.4, unit: 'mm', limit: '>= 2.0', pass: true },
    { quantity: '$K_t$ at the fillet', value: '1.8', source: 'Peterson, chart 3.1' },
  ],
}

test('a measured-values table validates; values may be numbers or exact text', () => {
  const card = parsed('measures', MEASURES)
  assert.equal(card.rows.length, 3)
  assert.equal(card.rows[2].value, '1.8')
})

test('an empty table is refused', () => {
  assert.equal(parseCard('measures', JSON.stringify({ rows: [] })).ok, false)
})

test('measures verdict: any out-of-limit row fails; an undecided check is not a pass', () => {
  // A row with no `pass` is a reported value, not a check; it does not vote.
  assert.equal(lineVerdict(fenced('measures', MEASURES)), 'pass')
  const undecided = { rows: [...MEASURES.rows, { quantity: 'Flatness', value: 'not measurable', pass: null }] }
  assert.equal(lineVerdict(fenced('measures', undecided)), 'inconclusive')
  const judged = { rows: MEASURES.rows.slice(0, 2) }
  judged.rows[1] = { ...judged.rows[1], pass: false }
  assert.equal(lineVerdict(fenced('measures', judged)), 'fail')
  assert.equal(lineVerdict(fenced('measures', { rows: [{ quantity: 'Mass', value: 2.4, unit: 'kg' }] })), null)
})
