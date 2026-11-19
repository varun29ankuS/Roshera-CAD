// Run: node --experimental-strip-types --test test/drawings-for-document.test.mjs
//
// Found rehearsing the demo video (2026-11-18), reported by Varun verbatim:
// "drawing 7ba3da4f-... exists, but the document/part it was created against
// does not resolve against the server's current live state". On load the app
// lists documents, THEN creates the fresh one and switches to it -- so the
// list it marks "active" in does not contain the new document, nothing is
// active, and the sheet filter's "active unknown -> keep everything" kept the
// previous document's drawing and opened it. The filter now takes the
// document from the app's own record of the switch (document-scope), and when
// that is unknown it keeps NO document-owned drawing (fail closed).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { drawingsForDocument } from '../src/lib/drawings-for-document.ts'

const here = dirname(fileURLToPath(import.meta.url))
const mine = { id: 'a', owner: { kind: 'legacy', document_id: 'doc-new' } }
const old = { id: '7ba3da4f', owner: { kind: 'legacy', document_id: 'doc-old' } }
const partOwned = { id: 'p', owner: { kind: 'part', part_id: 'uuid-1' } }

test('only the open document\'s drawings are listed', () => {
  assert.deepEqual(drawingsForDocument([mine, old], 'doc-new').map((d) => d.id), ['a'])
})

test('an unknown open document lists no document-owned drawing, never all of them', () => {
  assert.deepEqual(drawingsForDocument([mine, old], null).map((d) => d.id), [])
})

test('part-owned drawings are kept (they carry no document; the server binds them)', () => {
  assert.deepEqual(drawingsForDocument([partOwned, old], 'doc-new').map((d) => d.id), ['p'])
})

test('the Drawing workspace uses it with the app\'s own record of the open document', () => {
  const src = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')
  assert.ok(src.includes('drawingsForDocument('), 'the filter is the tested one')
  assert.ok(src.includes('currentDocument()'), 'from document-scope, set when the switch is confirmed')
})
