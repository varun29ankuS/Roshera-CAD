// Run: node --experimental-strip-types --test test/shared-fetch.test.mjs
//
// Five panels read the document's chapters (status bar, Blackboard, feature
// tree, timeline strip, feature locations). Each asked the server on its own,
// so every model change and every tab switch sent the same
// GET /api/timeline/chapters/main four or five times at once -- and each one
// replays and certifies the history server-side. These pin that simultaneous
// readers share ONE request, and that a reader never gets an answer that
// started before the change it is reacting to.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createCoalescer } from '../src/lib/shared-fetch.ts'

function clock() {
  let t = 0
  return { now: () => t, set: (v) => { t = v } }
}

test('readers reacting to the same change share one request', async () => {
  const c = clock()
  const share = createCoalescer(c.now)
  let started = 0
  const start = () => { started += 1; return Promise.resolve(`answer ${started}`) }
  c.set(10) // the change every reader reacts to happened at t=0
  const a = share('main', 0, start)
  const b = share('main', 0, start)
  const d = share('main', 0, start)
  assert.equal(started, 1)
  assert.deepEqual(await Promise.all([a, b, d]), ['answer 1', 'answer 1', 'answer 1'])
})

test('a request started BEFORE a reader\'s change is not shared with it', async () => {
  const c = clock()
  const share = createCoalescer(c.now)
  let started = 0
  let release
  const slow = () => { started += 1; return new Promise((r) => { release = r }) }
  c.set(5)
  const first = share('main', 0, slow) // started at t=5
  c.set(20)
  // This reader reacts to a change at t=12: the request from t=5 cannot
  // have seen it.
  const second = share('main', 12, () => { started += 1; return Promise.resolve('fresh') })
  assert.equal(started, 2)
  assert.equal(await second, 'fresh')
  release('stale')
  assert.equal(await first, 'stale')
})

test('different keys never share', () => {
  const c = clock()
  const share = createCoalescer(c.now)
  let started = 0
  const start = () => { started += 1; return new Promise(() => {}) }
  share('main', 0, start)
  share('branch-2', 0, start)
  assert.equal(started, 2)
})

test('a finished request is not reused: the next change asks again', async () => {
  const c = clock()
  const share = createCoalescer(c.now)
  let started = 0
  const start = () => { started += 1; return Promise.resolve(started) }
  await share('main', 0, start)
  await share('main', 0, start)
  assert.equal(started, 2)
})

test('a failed request fails every reader sharing it, and the next one retries', async () => {
  const c = clock()
  const share = createCoalescer(c.now)
  let started = 0
  const boom = () => { started += 1; return Promise.reject(new Error('HTTP 500')) }
  const a = share('main', 0, boom)
  const b = share('main', 0, boom)
  await assert.rejects(a, /HTTP 500/)
  await assert.rejects(b, /HTTP 500/)
  await assert.rejects(share('main', 0, boom), /HTTP 500/)
  assert.equal(started, 2)
})
