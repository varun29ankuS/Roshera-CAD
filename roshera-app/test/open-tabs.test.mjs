// Run: node --experimental-strip-types --test test/open-tabs.test.mjs
//
// Varun (2026-11-18): "there are few 10's of tabs open". The tab strip showed
// EVERY document ever made (33 on this server) except the ones closed in that
// particular browser, so any fresh browser opened onto a wall of tabs. Now it
// shows only the documents opened in this browser, plus the active one;
// everything else stays one click away under +.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  OPEN_TABS_KEY,
  hiddenDocuments,
  readOpenIds,
  visibleTabs,
  withClosed,
  withOpened,
  writeOpenIds,
} from '../src/lib/open-tabs.ts'

const here = dirname(fileURLToPath(import.meta.url))
const docs = [
  { id: 'a', active: false, createdAt: 1 },
  { id: 'b', active: true, createdAt: 2 },
  { id: 'c', active: false, createdAt: 3 },
]

test('a fresh browser shows only the active document', () => {
  assert.deepEqual(visibleTabs(docs, new Set()).map((d) => d.id), ['b'])
  assert.deepEqual(hiddenDocuments(docs, new Set()).map((d) => d.id), ['c', 'a'], 'the rest are under +, newest first')
})

test('opened documents stay as tabs until closed', () => {
  const open = withOpened(withOpened(new Set(), 'a'), 'b')
  assert.deepEqual(visibleTabs(docs, open).map((d) => d.id), ['a', 'b'])
  const closed = withClosed(open, 'a')
  assert.deepEqual(visibleTabs(docs, closed).map((d) => d.id), ['b'])
  assert.deepEqual(visibleTabs(docs, withClosed(open, 'b')).map((d) => d.id), ['a', 'b'], 'the active document never vanishes')
})

test('the open set survives a reload and broken storage', () => {
  const m = new Map()
  const s = { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)) }
  writeOpenIds(s, new Set(['a', 'c']))
  assert.deepEqual([...readOpenIds(s)].sort(), ['a', 'c'])
  s.setItem(OPEN_TABS_KEY, 'not json')
  assert.equal(readOpenIds(s).size, 0)
  const broken = { getItem: () => { throw new Error('x') }, setItem: () => { throw new Error('x') } }
  assert.equal(readOpenIds(broken).size, 0)
  assert.doesNotThrow(() => writeOpenIds(broken, new Set(['a'])))
})

test('the strip uses the open set, not the old closed-tabs list', () => {
  const src = readFileSync(join(here, '..', 'src', 'components', 'layout', 'DocumentTabs.tsx'), 'utf8')
  assert.ok(src.includes('visibleTabs('), 'tabs come from visibleTabs')
  assert.ok(!src.includes('tabs.closed.v1'), 'the closed-tabs list is gone')
  assert.ok(/useDocumentStore\.subscribe\([\s\S]*?withOpened\(prev, activeId\)/.test(src), 'the document you switch to joins the strip')
})
