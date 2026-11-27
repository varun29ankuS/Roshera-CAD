import { test } from 'node:test'
import assert from 'node:assert/strict'

const { shouldPrint, printSchedule, lineStarts, PRINT_CAP_MS, PRINT_STEP_MS } = await import('../src/lib/print-reveal.ts')

test('an entry that has just arrived prints; history already on the board does not', () => {
  const now = 1_000_000
  assert.equal(shouldPrint({ createdAt: now - 200, streaming: false }, now, false), true)
  assert.equal(shouldPrint({ createdAt: now - 60_000, streaming: false }, now, false), false, 'a minute old: history')
  assert.equal(shouldPrint({ createdAt: now - 86_400_000, streaming: false }, now, false), false, 'yesterday: history')
})

test('an entry still being written prints whatever its timestamp', () => {
  assert.equal(shouldPrint({ createdAt: 0, streaming: true }, 1_000_000, false), true)
})

test('a clock that disagrees does not turn history into news', () => {
  // A line stamped in the future (server clock ahead) is not "just arrived".
  assert.equal(shouldPrint({ createdAt: 2_000_000, streaming: false }, 1_000_000, false), false)
  assert.equal(shouldPrint({ createdAt: NaN, streaming: false }, 1_000_000, false), false)
})

test('reduced motion never prints', () => {
  assert.equal(shouldPrint({ createdAt: 999_900, streaming: false }, 1_000_000, true), false)
  assert.equal(shouldPrint({ createdAt: 0, streaming: true }, 1_000_000, true), false)
})

test('a short card prints at the fixed line interval', () => {
  const s = printSchedule(5)
  assert.equal(s.stepMs, PRINT_STEP_MS)
  assert.ok(PRINT_STEP_MS >= 30 && PRINT_STEP_MS <= 40)
  assert.ok(s.totalMs <= PRINT_CAP_MS)
})

test('a whole card is done within the cap however many lines it has', () => {
  for (const n of [1, 10, 14, 40, 400]) {
    const s = printSchedule(n)
    assert.ok(s.totalMs <= PRINT_CAP_MS, `${n} lines took ${s.totalMs} ms`)
    assert.ok(s.stepMs > 0, 'lines still arrive one after another')
  }
  assert.ok(PRINT_CAP_MS <= 500)
})

test('each block starts after every line of the blocks before it', () => {
  // header, a 10-row certificate table, a verdict line, a key/value block
  assert.deepEqual(lineStarts([1, 10, 1, 1]), { starts: [0, 1, 11, 12], total: 13 })
  assert.deepEqual(lineStarts([]), { starts: [], total: 0 })
})
