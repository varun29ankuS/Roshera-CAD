// Run: node --experimental-strip-types --test test/workspace-switch.test.mjs
//
// Varun (2026-11-18): "it hangs every time i change the tab when it should be
// instant". Measured: going back to Part blocked the main thread 11-14 s,
// because App.tsx UNMOUNTED the 3D viewport on Drawing / Assembly and rebuilt
// it (every part's mesh) on return. The Part workspace now stays mounted and
// is only hidden; its own Blackboard is not mounted while another workspace
// docks one (two would both poll and both own a composer).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const app = readFileSync(join(here, '..', 'src', 'App.tsx'), 'utf8')

test('the 3D viewport is never unmounted by a workspace switch', () => {
  assert.ok(
    !/\?\s*\(\s*<>\s*\{docMode === 'drawing' \? <DrawingsWorkspace \/> : <AssemblyWorkspace \/>\}[\s\S]*?\)\s*:\s*\(/.test(app),
    'the Part workspace is not the else-branch of a drawing/assembly ternary',
  )
  assert.ok(/hidden=\{sheetWorkspace\}/.test(app), 'the Part workspace is hidden, not unmounted, on other tabs')
  assert.equal((app.match(/<CADViewport \/>/g) ?? []).length, 1, 'one viewport, always mounted')
})

test('Drawing and Assembly stay mounted after their first visit', () => {
  // Assembly rebuilt its 3D view from every part's mesh on each entry: a
  // 15.9 s freeze measured live.
  assert.ok(/visited\.assembly && \(\s*<div hidden=\{docMode !== 'assembly'\}/.test(app), 'Assembly is hidden, not unmounted')
  assert.ok(/visited\.drawing && \(\s*<div hidden=\{docMode !== 'drawing'\}/.test(app), 'Drawing is hidden, not unmounted')
  assert.equal((app.match(/<AssemblyWorkspace \/>/g) ?? []).length, 1)
  assert.equal((app.match(/<DrawingsWorkspace \/>/g) ?? []).length, 1)
})

test('one Blackboard at a time', () => {
  assert.ok(/!sheetWorkspace && \(\s*<div[^>]*>\s*<Blackboard docked \/>/.test(app), "the Part workspace's Blackboard yields to the docked one")
})
