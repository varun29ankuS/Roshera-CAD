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
