// Run: node --experimental-strip-types --test test/document-scope.test.mjs
//
// A document's history is numbered from its own step 0: two documents both
// have a "step 3 on main". Every client cache of history -- the film strip's
// pictures, the scrubbed meshes -- used to be keyed `branch:step` only, so
// after a tab switch the strip showed the PREVIOUS document's pictures under
// the new document's steps (Varun 2026-11-15: "timeline doesn't update with
// tab"). These pin that a document change drops the old history, and that an
// answer for the old document arriving late never lands in the new one.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  DocumentScopedCache,
  currentDocument,
  enterDocument,
  onDocumentChange,
  isCurrentScope,
  scopeToken,
} from '../src/lib/document-scope.ts'

test('a step cached for one document is not served in another', () => {
  enterDocument('doc-a')
  const cache = new DocumentScopedCache(8)
  cache.set('main:3', 'picture of A step 3')
  assert.equal(cache.get('main:3'), 'picture of A step 3')
  enterDocument('doc-b')
  assert.equal(cache.get('main:3'), undefined)
  assert.equal(cache.size, 0)
})

test('dropping the old document hands back what was dropped (object URLs get revoked)', () => {
  enterDocument('doc-a')
  const dropped = []
  const cache = new DocumentScopedCache(8, (v) => dropped.push(v))
  cache.set('main:0', 'blob:a0')
  cache.set('main:1', 'blob:a1')
  enterDocument('doc-b')
  assert.deepEqual(dropped.sort(), ['blob:a0', 'blob:a1'])
})

test('an answer for the old document arriving after the switch is not kept', () => {
  enterDocument('doc-a')
  const cache = new DocumentScopedCache(8)
  const token = scopeToken()
  enterDocument('doc-b')
  // The fetch was started in doc-a and resolves now, in doc-b.
  cache.setIfCurrent(token, 'main:3', 'late picture of A')
  assert.equal(cache.get('main:3'), undefined)
  assert.equal(isCurrentScope(token), false)
})

test('re-entering the same document keeps its history; a different one clears it', () => {
  enterDocument('doc-a')
  const cache = new DocumentScopedCache(8)
  cache.set('main:2', 'A2')
  enterDocument('doc-a')
  assert.equal(cache.get('main:2'), 'A2')
  assert.equal(currentDocument(), 'doc-a')
})

test('re-opening a document is a new scope even with the same id (its history was replayed)', () => {
  enterDocument('doc-a')
  const token = scopeToken()
  enterDocument('doc-a', { reopened: true })
  assert.equal(isCurrentScope(token), false)
})

test('subscribers hear every document change exactly once', () => {
  enterDocument('doc-a')
  const heard = []
  const off = onDocumentChange((id) => heard.push(id))
  enterDocument('doc-b')
  enterDocument('doc-b')
  enterDocument('doc-c')
  off()
  enterDocument('doc-d')
  assert.deepEqual(heard, ['doc-b', 'doc-c'])
})

test('the cache stays bounded, oldest first out', () => {
  enterDocument('doc-a')
  const dropped = []
  const cache = new DocumentScopedCache(2, (v) => dropped.push(v))
  cache.set('k1', 'v1')
  cache.set('k2', 'v2')
  cache.get('k1') // most recently used now
  cache.set('k3', 'v3')
  assert.equal(cache.get('k2'), undefined)
  assert.deepEqual(dropped, ['v2'])
})
