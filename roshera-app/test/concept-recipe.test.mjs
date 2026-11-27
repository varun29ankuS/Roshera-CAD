// Run: node --experimental-strip-types --test test/concept-recipe.test.mjs
//
// The `roshera:concept` card (concept board, spec 2026-11-27): a set of up to
// three concept sketches, each a small SDF recipe whose primitives map one to
// one onto kernel verbs. A payload the schema rejects must render as the raw
// fence with its error -- never as a half card -- so the validation is the
// honesty: a typo'd radius, a non-unit axis or a blend the fillet verb could
// not make are refused here, not drawn as something they are not.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { BRACKET_SET, clone } from './concept-fixtures.mjs'

const { parseCard, CARD_KINDS, cardKindFromLanguage } = await import('../src/lib/blackboard-cards.ts')
const { buildRequestText, conceptNumberText } = await import('../src/lib/concept-build.ts')
const { cardVerdict } = await import('../src/lib/blackboard-line-state.ts')

const here = dirname(fileURLToPath(import.meta.url))

function parse(payload) {
  return parseCard('concept', JSON.stringify(payload))
}
function rejects(payload, pathRe) {
  const r = parse(payload)
  assert.equal(r.ok, false, 'a bad payload must not validate')
  if (pathRe) assert.match(r.error, pathRe)
}

test('concept is a card kind the board renders', () => {
  assert.ok(CARD_KINDS.includes('concept'))
  assert.equal(cardKindFromLanguage('language-roshera:concept'), 'concept')
})

test('the three-concept bracket set validates', () => {
  const r = parse(BRACKET_SET)
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  assert.equal(r.card.card.concepts.length, 3)
  // Defaults the kernel verbs also default: +Z axis, the XY frame.
  const bore = r.card.card.concepts[1].recipe.steps[1].shape
  assert.deepEqual(bore.axis, [0, 0, 1])
  const box = r.card.card.concepts[0].recipe.steps[0].shape
  assert.deepEqual(box.u_axis, [1, 0, 0])
  assert.deepEqual(box.v_axis, [0, 1, 0])
})

test('one to three concepts, ids unique', () => {
  rejects({ concepts: [] }, /concepts/)
  const four = clone(BRACKET_SET)
  four.concepts.push({ ...clone(BRACKET_SET.concepts[2]), id: 'D' })
  rejects(four, /concepts/)
  const dup = clone(BRACKET_SET)
  dup.concepts[1].id = 'A'
  rejects(dup, /unique/)
})

test('an unknown field is refused, not silently dropped (a typo is wrong geometry)', () => {
  const s = clone(BRACKET_SET)
  s.concepts[0].recipe.steps[4].shape.raduis = 11.5
  rejects(s)
  const t = clone(BRACKET_SET)
  t.concepts[0].colour = 'red'
  rejects(t)
})

test('sizes must be positive and finite', () => {
  const s = clone(BRACKET_SET)
  s.concepts[0].recipe.steps[0].shape.width = 0
  rejects(s, /width/)
  const t = clone(BRACKET_SET)
  t.concepts[0].recipe.steps[0].shape.height = -5
  rejects(t, /height/)
})

test('axes are unit vectors and frames orthonormal (the kernel frames require it)', () => {
  const s = clone(BRACKET_SET)
  s.concepts[0].recipe.steps[4].shape.axis = [0, 2, 0]
  rejects(s, /unit/)
  const t = clone(BRACKET_SET)
  t.concepts[0].recipe.steps[0].shape.u_axis = [1, 0, 0]
  t.concepts[0].recipe.steps[0].shape.v_axis = [0.6, 0.8, 0]
  rejects(t, /orthogonal/)
  const u = clone(BRACKET_SET)
  u.concepts[1].recipe.steps[2].shape.dir = [0, 0, 1]
  rejects(u, /orthogonal/)
})

test('the recipe starts from a body: the first step is a plain union', () => {
  const s = clone(BRACKET_SET)
  s.concepts[0].recipe.steps[0].op = 'subtract'
  rejects(s, /first step/)
  const t = clone(BRACKET_SET)
  t.concepts[0].recipe.steps[0].blend = 2
  rejects(t, /first step/)
})

test('a blend radius is allowed on a union only (the fillet verb rounds a junction)', () => {
  const s = clone(BRACKET_SET)
  s.concepts[0].recipe.steps[4].blend = 2
  rejects(s, /blend/)
})

test('shape-specific limits: plate corners, slot length, rib legs, cone radii', () => {
  const plate = clone(BRACKET_SET)
  plate.concepts[1].recipe.steps[0].shape.corner_radius = 30
  rejects(plate, /corner_radius/)
  const slot = clone(BRACKET_SET)
  slot.concepts[1].recipe.steps[2].shape.length = 5
  rejects(slot, /length/)
  const rib = clone(BRACKET_SET)
  rib.concepts[0].recipe.steps[2].shape.leg_b = [0, 60, 0]
  rejects(rib, /leg/)
  const cone = clone(BRACKET_SET)
  cone.concepts[2].recipe.steps[1].shape.base_radius = 0
  cone.concepts[2].recipe.steps[1].shape.top_radius = 0
  rejects(cone, /radius/)
})

test('loads need supports and supports need loads (the load path joins them)', () => {
  const s = clone(BRACKET_SET)
  delete s.concepts[0].supports
  rejects(s, /supports/)
  const t = clone(BRACKET_SET)
  t.concepts[0].loads[0].dir = [0, 0, 0]
  rejects(t, /dir/)
})

test('every number the agent sends says how it was obtained', () => {
  const s = clone(BRACKET_SET)
  delete s.concepts[0].numbers[0].basis
  rejects(s, /basis/)
})

test('steps are capped at the shader loop bound', () => {
  const s = clone(BRACKET_SET)
  const step = s.concepts[0].recipe.steps[4]
  while (s.concepts[0].recipe.steps.length <= 12) s.concepts[0].recipe.steps.push(clone(step))
  rejects(s, /steps/)
})

test('a concept never colours the line marker: it is a proposal, not a verdict', () => {
  const r = parse(BRACKET_SET)
  assert.equal(cardVerdict(r.card), null)
})

test('the Build request names the concept and its set, and asks for the kernel build + certificate', () => {
  const r = parse(BRACKET_SET)
  const set = r.card.card
  const text = buildRequestText(set, set.concepts[1])
  assert.match(text, /^Build concept B/)
  assert.ok(text.includes('Flat plate on standoffs'))
  assert.ok(text.includes('motor-bracket'))
  assert.match(text, /certif/i)
  // Mutation control: a different concept gives a different request.
  assert.notEqual(buildRequestText(set, set.concepts[0]), text)
})

test('every number reads as an estimate with its basis', () => {
  const text = conceptNumberText({ label: 'tip deflection', value: 0.02, unit: 'mm', basis: 'beam theory' })
  assert.match(text, /estimate/)
  assert.match(text, /beam theory/)
  assert.match(text, /0\.02 mm/)
})

test('the policy teaches the concept card and the concepts-first rule', () => {
  const policy = readFileSync(join(here, '..', '..', 'roshera-agent-policy.md'), 'utf8')
  assert.match(policy, /`concept`/)
  assert.ok(policy.includes('kb_lookup(kind:"playbook", key:"concepts")'))
  assert.match(policy, /three concepts/i)
  assert.match(policy, /show,?\s+(it,?\s+)?not\s+tell/i)
})

test('the concept fixtures in the playbook validate as cards (one example, two packages)', () => {
  // roshera-mcp's concepts playbook carries the same bracket set as JSON
  // between two markers; the app must accept exactly what the agent is taught.
  const src = readFileSync(join(here, '..', '..', 'roshera-mcp', 'src', 'tools', 'kb_concepts.ts'), 'utf8')
  const m = /\/\* concept-example:begin \*\/([\s\S]*?)\/\* concept-example:end \*\//.exec(src)
  assert.ok(m, 'the playbook example markers are present')
  const r = parseCard('concept', m[1])
  assert.equal(r.ok, true, r.ok ? '' : r.error)
})
