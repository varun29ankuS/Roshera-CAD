import { test } from 'node:test'
import assert from 'node:assert/strict'

const { WORKSPACES, parseWorkspace } = await import('../src/lib/workspaces.ts')

test('the workspaces, in tab order, with the names engineers know', () => {
  assert.deepEqual(
    WORKSPACES.map((w) => [w.id, w.label]),
    [
      ['part', 'Part'],
      ['assembly', 'Assembly'],
      ['drawing', 'Drawing'],
      ['dfm', 'DFM'],
    ],
  )
})

test('every workspace id round-trips; anything else is refused', () => {
  for (const w of WORKSPACES) assert.equal(parseWorkspace(w.id), w.id)
  assert.equal(parseWorkspace('PART'), 'part', 'case-insensitive, as a URL hash may be')
  for (const junk of ['', 'simulate', 'modeling', null, undefined, 3]) {
    assert.equal(parseWorkspace(junk), null, String(junk))
  }
})
