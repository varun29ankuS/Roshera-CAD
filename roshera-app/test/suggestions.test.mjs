import { test } from 'node:test'
import assert from 'node:assert/strict'

const { suggestPrompts } = await import('../src/lib/suggestions.ts')

const ch = (kind, name = null) => ({ kind, name, events: [0], anchor: 0, sound: true, refusals: [], author: 'user', author_detail: { kind: 'user' }, intent: null, instances: [] })

test('an empty document suggests starting a part', () => {
  const s = suggestPrompts([], [])
  assert.equal(s.length, 3)
  assert.match(s[0], /flange|bracket|part/i)
})

test('right after an edit, the first suggestion checks the part', () => {
  const s = suggestPrompts([ch('cylinder', 'Flange'), ch('edit', 'Edit')], [])
  assert.match(s[0], /after my last edit/)
})

test('holes with no chamfer, and a boss with no fillet, suggest finishing them -- by their names', () => {
  const s = suggestPrompts([ch('cylinder', 'Flange'), ch('boss', 'Hub'), ch('hole', 'Bore'), ch('pattern')], [])
  assert.ok(s.some((x) => /Fillet where the Hub meets the Flange/.test(x)), s.join(' | '))
  assert.ok(s.some((x) => /Chamfer the hole edges/.test(x)), s.join(' | '))
})

test('a part already filleted and chamfered is not told to be; with no drawing, a drawing is suggested', () => {
  const s = suggestPrompts([ch('cylinder', 'Flange'), ch('boss', 'Hub'), ch('hole', 'Bore'), ch('fillet'), ch('chamfer')], [])
  assert.ok(!s.some((x) => /Fillet where|Chamfer the hole/.test(x)), s.join(' | '))
  assert.ok(s.some((x) => /drawing/i.test(x)), s.join(' | '))
})

test('with a drawing already made, no drawing is suggested', () => {
  const s = suggestPrompts([ch('cylinder', 'Flange'), ch('fillet'), ch('chamfer')], [ch('drawing.create_from_part')])
  assert.ok(!s.some((x) => /drawing/i.test(x)), s.join(' | '))
  assert.equal(s.length, 3)
})

test('an assembly made from the part is not a drawing: a drawing is still suggested', () => {
  const s = suggestPrompts([ch('cylinder', 'Flange'), ch('fillet'), ch('chamfer')], [ch('assembly.create')])
  assert.ok(s.some((x) => /drawing/i.test(x)), s.join(' | '))
})
