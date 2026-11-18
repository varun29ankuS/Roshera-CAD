// Run: node --experimental-strip-types --test test/latest-only.test.mjs
//
// Found rehearsing the demo video (2026-11-18): a fresh document showed the
// six parts of the document the server had open before it. On load the app
// asks for the scene of whatever is open (a large document: a slow answer),
// then switches to the fresh blank (an empty scene: a fast answer). The slow
// answer landed LAST and overwrote the empty scene -- and because the blank
// then held parts, the next load thought it had been used and made another.
// Only the newest scene request may write the scene.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { latestOnly } from '../src/lib/latest-only.ts'

const here = dirname(fileURLToPath(import.meta.url))
const wait = (ms, v) => new Promise((r) => setTimeout(() => r(v), ms))

test('a slow old answer that lands after a newer one is dropped', async () => {
  const gate = latestOnly()
  let scene = 'start'
  const fetchScene = async (answer, ms) => {
    const ticket = gate.begin()
    const got = await wait(ms, answer)
    if (gate.isLatest(ticket)) scene = got
  }
  const old = fetchScene('six parts of the old document', 60)
  const fresh = fetchScene('empty fresh document', 5)
  await Promise.all([old, fresh])
  assert.equal(scene, 'empty fresh document')
})

test('answers in order are all applied, the last one winning', async () => {
  const gate = latestOnly()
  const applied = []
  const t1 = gate.begin()
  if (gate.isLatest(t1)) applied.push(1)
  const t2 = gate.begin()
  if (gate.isLatest(t2)) applied.push(2)
  assert.deepEqual(applied, [1, 2])
  assert.equal(gate.isLatest(t1), false)
})

test('the scene refresh only writes the newest answer', () => {
  const src = readFileSync(join(here, '..', 'src', 'lib', 'ws-bridge.ts'), 'utf8')
  const start = src.indexOf('async function resyncSceneFromServer')
  const body = src.slice(start, src.indexOf('scene.clearScene()', start))
  assert.ok(/sceneRequests\.begin\(\)/.test(body), 'each scene request takes a ticket')
  // The decisive check is the LAST one before the scene is replaced: a newer
  // request can start while this one's body is still arriving and converting.
  const afterConvert = body.slice(body.indexOf('const converted'))
  assert.ok(/if \(!sceneRequests\.isLatest\(ticket\)\) return/.test(afterConvert), 'a stale answer is dropped right before the scene is replaced')
})
