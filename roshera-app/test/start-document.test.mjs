// Run: node --experimental-strip-types --test test/start-document.test.mjs
//
// Varun (2026-11-18): "it should start with a new document everytime ...
// unless someone saves something and opens an old one". The app used to
// reopen the last document on every load, which replays and re-certifies
// its whole history (minutes on a document with STEP imports). Now a load
// starts in a fresh document: the blank one this browser made last time if
// nothing was built in it (so blanks do not pile up), else a new one. Old
// documents are opened on purpose, from the tab strip's + menu.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  planStartDocument,
  readStartDocument,
  writeStartDocument,
  markStartDocumentUsed,
  START_DOCUMENT_KEY,
} from '../src/lib/start-document.ts'

const here = dirname(fileURLToPath(import.meta.url))

function memoryStorage() {
  const m = new Map()
  return { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)) }
}
const docs = (...ids) => ids.map((id, i) => ({ id, active: i === 0 }))

test('a first load creates a fresh document', () => {
  assert.deepEqual(planStartDocument(docs('main'), null), { kind: 'create' })
})

test("an unused blank from last time is reused, not another one made", () => {
  assert.deepEqual(planStartDocument(docs('main', 'blank'), { id: 'blank', used: false }), { kind: 'open', id: 'blank' })
  assert.deepEqual(planStartDocument(docs('blank', 'main'), { id: 'blank', used: false }), { kind: 'stay' })
})

test('a blank that was built in, or that no longer exists, gives a new one', () => {
  assert.deepEqual(planStartDocument(docs('main', 'blank'), { id: 'blank', used: true }), { kind: 'create' })
  assert.deepEqual(planStartDocument(docs('main'), { id: 'gone', used: false }), { kind: 'create' })
})

test('the blank is remembered and marked used only when it is the one in use', () => {
  const s = memoryStorage()
  writeStartDocument(s, { id: 'blank', used: false })
  assert.deepEqual(readStartDocument(s), { id: 'blank', used: false })
  markStartDocumentUsed(s, 'other')
  assert.equal(readStartDocument(s).used, false, 'building in another document does not use up the blank')
  markStartDocumentUsed(s, 'blank')
  assert.equal(readStartDocument(s).used, true)
  s.setItem(START_DOCUMENT_KEY, '{not json')
  assert.equal(readStartDocument(s), null, 'a corrupt entry reads as none')
  const broken = { getItem: () => { throw new Error('blocked') }, setItem: () => { throw new Error('blocked') } }
  assert.equal(readStartDocument(broken), null)
  assert.doesNotThrow(() => writeStartDocument(broken, { id: 'x', used: false }))
})

test('the store starts with the plan, never by reopening the last document', () => {
  const store = readFileSync(join(here, '..', 'src', 'stores', 'document-store.ts'), 'utf8')
  assert.ok(store.includes('planStartDocument('), 'startup asks planStartDocument')
  assert.ok(!store.includes('lastDocumentId'), 'the reopen-last-document memory is gone')
  assert.ok(store.includes('markStartDocumentUsed('), 'building in the blank marks it used')
})
