import { test } from 'node:test'
import assert from 'node:assert/strict'

const { deleteParts } = await import('../src/lib/delete-parts.ts')

const parts = [
  { id: 'a', name: 'bracket' },
  { id: 'b', name: 'cover plate' },
]

test('a delete the kernel accepts says nothing: the part leaving the screen is the answer', async () => {
  const lines = []
  const asked = []
  const n = await deleteParts(parts, async (id) => (asked.push(id), { ok: true, status: 200 }), (l) => lines.push(l))
  assert.equal(n, 2)
  assert.deepEqual(asked, ['a', 'b'])
  assert.deepEqual(lines, [])
})

test('a delete the kernel refuses is reported by name, with the status', async () => {
  const lines = []
  const n = await deleteParts(
    parts,
    async (id) => (id === 'b' ? { ok: false, status: 409 } : { ok: true, status: 200 }),
    (l) => lines.push(l),
  )
  assert.equal(n, 1)
  assert.equal(lines.length, 1)
  assert.match(lines[0], /cover plate/)
  assert.match(lines[0], /409/)
  assert.match(lines[0], /still in the document/)
})

test('a kernel that cannot be reached is reported, and the rest are still tried', async () => {
  const lines = []
  const asked = []
  const n = await deleteParts(
    parts,
    async (id) => {
      asked.push(id)
      if (id === 'a') throw new Error('network down')
      return { ok: true, status: 200 }
    },
    (l) => lines.push(l),
  )
  assert.equal(n, 1)
  assert.deepEqual(asked, ['a', 'b'])
  assert.equal(lines.length, 1)
  assert.match(lines[0], /bracket/)
  assert.match(lines[0], /could not be reached/)
})

test('nothing selected asks the kernel nothing', async () => {
  let asked = 0
  const n = await deleteParts([], async () => (asked++, { ok: true, status: 200 }), () => assert.fail('no report'))
  assert.equal(n, 0)
  assert.equal(asked, 0)
})
