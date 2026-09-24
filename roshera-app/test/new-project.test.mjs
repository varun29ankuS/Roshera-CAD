// Run: node --experimental-strip-types --test test/new-project.test.mjs
//
// New Project creates and opens a fresh backend document. The local scene is
// left alone until the backend has done that; a refusal is shown and the
// scene the user was looking at stays on screen.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { runNewProject } from '../src/lib/new-project.ts'

// That the palette's New Project does not clear the scene first is asserted
// against the component source in `client-messages.test.mjs`.
test('a backend refusal is reported with its message', async () => {
  const reports = []
  const ok = await runNewProject(
    async () => {
      throw new Error('createDocument: document limit reached')
    },
    (line) => reports.push(line),
  )
  assert.equal(ok, false)
  assert.equal(reports.length, 1)
  assert.match(reports[0], /document limit reached/)
  assert.ok(!reports[0].includes('  '))
})

test('a transport failure is reported as unreachable', async () => {
  const reports = []
  const ok = await runNewProject(
    async () => {
      throw new TypeError('Failed to fetch')
    },
    (line) => reports.push(line),
  )
  assert.equal(ok, false)
  assert.match(reports[0], /backend unreachable/)
})

test('success reports nothing (the page reloads onto the new document)', async () => {
  const reports = []
  const ok = await runNewProject(async () => {}, (line) => reports.push(line))
  assert.equal(ok, true)
  assert.equal(reports.length, 0)
})
