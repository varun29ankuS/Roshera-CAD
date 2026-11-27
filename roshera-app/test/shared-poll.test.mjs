import { test } from 'node:test'
import assert from 'node:assert/strict'

const { createSharedPoll } = await import('../src/lib/shared-poll.ts')

/** A fake clock and tab: timers that fire when told, visibility that flips. */
function harness(values) {
  let loads = 0
  let visible = true
  const timers = new Set()
  const visibilityListeners = new Set()
  const poll = createSharedPoll({
    load: async () => values[Math.min(loads++, values.length - 1)],
    intervalMs: 5000,
    setInterval: (fn) => {
      timers.add(fn)
      return fn
    },
    clearInterval: (h) => timers.delete(h),
    isVisible: () => visible,
    onVisibilityChange: (fn) => {
      visibilityListeners.add(fn)
      return () => visibilityListeners.delete(fn)
    },
  })
  const settle = () => new Promise((r) => setTimeout(r, 0))
  return {
    poll,
    loads: () => loads,
    timers: () => timers.size,
    tick: async () => {
      for (const fn of [...timers]) fn()
      await settle()
    },
    setVisible: async (v) => {
      visible = v
      for (const fn of [...visibilityListeners]) fn()
      await settle()
    },
    settle,
  }
}

test('many readers of one endpoint share one request and one timer', async () => {
  const h = harness(['a', 'b'])
  const seenA = []
  const seenB = []
  const offA = h.poll.subscribe((v) => seenA.push(v))
  const offB = h.poll.subscribe((v) => seenB.push(v))
  await h.settle()
  assert.equal(h.loads(), 1, 'two subscribers, one request')
  assert.equal(h.timers(), 1, 'and one timer')
  assert.deepEqual(seenA, ['a'])
  assert.deepEqual(seenB, ['a'])
  await h.tick()
  assert.equal(h.loads(), 2)
  assert.deepEqual(seenA, ['a', 'b'])
  offA()
  offB()
})

test('a reader that arrives later gets what is already known, at once', async () => {
  const h = harness(['a'])
  const off = h.poll.subscribe(() => {})
  await h.settle()
  const late = []
  const offLate = h.poll.subscribe((v) => late.push(v))
  assert.deepEqual(late, ['a'])
  assert.equal(h.loads(), 1, 'without asking again')
  off()
  offLate()
})

test('the timer stops with the last reader', async () => {
  const h = harness(['a'])
  const off1 = h.poll.subscribe(() => {})
  const off2 = h.poll.subscribe(() => {})
  await h.settle()
  off1()
  assert.equal(h.timers(), 1)
  off2()
  assert.equal(h.timers(), 0)
})

test('a hidden tab asks nothing; coming back asks at once', async () => {
  const h = harness(['a', 'b', 'c'])
  const seen = []
  const off = h.poll.subscribe((v) => seen.push(v))
  await h.settle()
  await h.setVisible(false)
  await h.tick()
  await h.tick()
  assert.equal(h.loads(), 1, 'no requests while hidden')
  await h.setVisible(true)
  assert.equal(h.loads(), 2, 'one on return')
  assert.deepEqual(seen, ['a', 'b'])
  off()
})

test('refresh asks now and tells every reader', async () => {
  const h = harness(['a', 'b'])
  const seen = []
  const off = h.poll.subscribe((v) => seen.push(v))
  await h.settle()
  await h.poll.refresh()
  assert.deepEqual(seen, ['a', 'b'])
  off()
})

test('a failed request keeps what was known and the next one still runs', async () => {
  let n = 0
  const timers = new Set()
  const poll = createSharedPoll({
    load: async () => {
      n += 1
      if (n === 2) throw new Error('backend down')
      return `v${n}`
    },
    intervalMs: 5000,
    setInterval: (fn) => (timers.add(fn), fn),
    clearInterval: (h) => timers.delete(h),
    isVisible: () => true,
    onVisibilityChange: () => () => {},
  })
  const seen = []
  const off = poll.subscribe((v) => seen.push(v))
  const settle = () => new Promise((r) => setTimeout(r, 0))
  await settle()
  for (const fn of [...timers]) fn()
  await settle()
  assert.deepEqual(seen, ['v1'], 'the failure changed nothing')
  for (const fn of [...timers]) fn()
  await settle()
  assert.deepEqual(seen, ['v1', 'v3'])
  off()
})
