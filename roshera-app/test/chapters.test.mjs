// Run: node --experimental-strip-types --test test/chapters.test.mjs
//
// The timeline's rows are chapters -- engineering features, the way a CAD
// feature tree reads (GET /api/timeline/chapters/{branch}). These pin how a
// chapter is titled and which icon it wears.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { chapterTitle, chapterIconKey, framePictureUrl } from '../src/lib/chapters-api.ts'

const ch = (over) => ({
  kind: 'hole',
  name: null,
  events: [3, 4],
  anchor: 4,
  sound: true,
  refusals: [],
  author: 'system',
  author_detail: { kind: 'system' },
  intent: null,
  instances: [],
  ...over,
})

test('a chapter is titled by its recorded name, else by what it is', () => {
  assert.equal(chapterTitle(ch({ name: 'Bolt hole 1 of 4' })), 'Bolt hole 1 of 4')
  assert.equal(chapterTitle(ch({})), 'Hole')
  assert.equal(chapterTitle(ch({ kind: 'boolean_difference' })), 'Boolean difference')
  assert.equal(
    chapterTitle(ch({ kind: 'pattern', instances: [ch({}), ch({}), ch({}), ch({})] })),
    'Hole pattern ×4',
  )
})

test('each chapter wears the icon of the tool that made it', () => {
  assert.equal(chapterIconKey('hole'), 'hole')
  assert.equal(chapterIconKey('cut'), 'subtract')
  assert.equal(chapterIconKey('boss'), 'union')
  assert.equal(chapterIconKey('revolve'), 'revolve')
  assert.equal(chapterIconKey('box'), 'box')
  assert.equal(chapterIconKey('pattern'), 'circularPattern')
  assert.equal(chapterIconKey('transform'), 'translate')
  assert.equal(chapterIconKey('something_new'), null, 'no icon is better than a wrong one')
})

test('a frame picture is addressed by branch and step', () => {
  assert.equal(framePictureUrl('main', 4, 128), '/api/timeline/frame/main/4/picture?size=128')
})

test("a drawing is the document's, not a feature of the part: it leaves the part's history", async () => {
  const { partChapters } = await import('../src/lib/chapters-api.ts')
  const cs = [ch({ kind: 'box', anchor: 0 }), ch({ kind: 'drawing.create_from_part', anchor: 1 }), ch({ kind: 'hole', anchor: 3 })]
  const { part, documents } = partChapters(cs)
  assert.deepEqual(part.map((c) => c.anchor), [0, 3])
  assert.deepEqual(documents.map((c) => c.anchor), [1])
})

test('an edit is not a feature: consecutive size edits fold into one "Edit" step, and the tree leaves it out', async () => {
  const { foldEdits, isEdit } = await import('../src/lib/chapters-api.ts')
  const ch = (kind, anchor, events) => ({ kind, name: null, events, anchor, sound: true, refusals: [], author: 'user', author_detail: { kind: 'user' }, intent: null, instances: [] })
  const folded = foldEdits([
    ch('cylinder', 0, [0, 1]),
    ch('pattern', 19, [8, 19]),
    ch('param.mould', 20, [20]),
    ch('param.mould', 21, [21]),
    ch('param.mould', 22, [22]),
    ch('fillet', 24, [24]),
    ch('param.mould', 25, [25]),
  ])
  assert.deepEqual(folded.map((c) => [c.kind, c.anchor, c.events]), [
    ['cylinder', 0, [0, 1]],
    ['pattern', 19, [8, 19]],
    ['edit', 22, [20, 21, 22]],
    ['fillet', 24, [24]],
    ['edit', 25, [25]],
  ])
  assert.deepEqual(folded.filter((c) => !isEdit(c)).map((c) => c.kind), ['cylinder', 'pattern', 'fillet'])
})

test('assembly steps are steps of the document, not features of the part', async () => {
  const { partChapters } = await import('../src/lib/chapters-api.ts')
  const ch = (kind) => ({ kind, name: null, events: [0], anchor: 0, sound: true, refusals: [], author: 'user', author_detail: { kind: 'user' }, intent: null, instances: [] })
  const { part, documents } = partChapters([ch('cylinder'), ch('assembly.create'), ch('assembly.add_instance'), ch('assembly.solve'), ch('drawing.create_from_part')])
  assert.deepEqual(part.map((c) => c.kind), ['cylinder'])
  assert.equal(documents.length, 4)
})

test('a deleted feature reads as "Delete" in the film strip; an edit as "Edit"', async () => {
  const { foldEdits } = await import('../src/lib/chapters-api.ts')
  const ch = (kind, anchor) => ({ kind, name: null, events: [anchor], anchor, sound: true, refusals: [], author: 'user', author_detail: { kind: 'user' }, intent: null, instances: [] })
  const folded = foldEdits([ch('cylinder', 0), ch('param.suppress', 5), ch('param.mould', 6)])
  assert.deepEqual(folded.map((c) => c.name), [null, 'Delete', 'Edit'])
})

test('the feature tree lists features, not edits or unused sketches', async () => {
  const { inFeatureTree } = await import('../src/lib/chapters-api.ts')
  const ch = (kind) => ({ kind, name: null, events: [1], anchor: 1, sound: true, refusals: [], author: 'user', author_detail: { kind: 'user' }, intent: null, instances: [] })
  assert.equal(inFeatureTree(ch('extrude')), true)
  assert.equal(inFeatureTree(ch('sketch')), false)
  assert.equal(inFeatureTree(ch('param.mould')), false)
})
