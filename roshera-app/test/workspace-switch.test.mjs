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
  assert.ok(/inert=\{sheetWorkspace\}[^>]*className=\{parked\(sheetWorkspace\)\}/.test(app), 'the Part workspace is parked, not unmounted, on other tabs')
  assert.equal((app.match(/<CADViewport \/>/g) ?? []).length, 1, 'one viewport, always mounted')
})

// Varun (2026-11-18): "why is part always so zoomed in whenever I shift to
// part / from drawing". A `hidden` (display: none) workspace measures 0 x 0,
// so its 3D canvas and camera framing were rebuilt from nothing on return.
// A workspace you leave keeps its full size -- laid behind the active one,
// invisible, out of reach of pointer and keyboard -- so nothing resizes.
test('a workspace you leave keeps its size: parked, never display:none', () => {
  assert.ok(!/<div hidden=\{/.test(app), 'no workspace is hidden with display:none')
  assert.ok(/'pointer-events-none invisible absolute inset-0'/.test(app), 'parked = full size, invisible, unclickable')
  assert.ok(/className="relative flex flex-1 min-h-0"/.test(app), 'the parked workspace fills the same area as the active one')
})

test('Drawing and Assembly stay mounted after their first visit', () => {
  // Assembly rebuilt its 3D view from every part's mesh on each entry: a
  // 15.9 s freeze measured live.
  assert.ok(/visited\.assembly && \(\s*<div[^>]*inert=\{docMode !== 'assembly'\}/.test(app), 'Assembly is parked, not unmounted')
  assert.ok(/visited\.drawing && \(\s*<div[^>]*inert=\{docMode !== 'drawing'\}/.test(app), 'Drawing is parked, not unmounted')
  assert.equal((app.match(/<AssemblyWorkspace \/>/g) ?? []).length, 1)
  assert.equal((app.match(/<DrawingsWorkspace \/>/g) ?? []).length, 1)
})

test('one Blackboard at a time', () => {
  assert.ok(/!sheetWorkspace && \(\s*<div[^>]*>\s*<Blackboard docked \/>/.test(app), "the Part workspace's Blackboard yields to the docked one")
})
