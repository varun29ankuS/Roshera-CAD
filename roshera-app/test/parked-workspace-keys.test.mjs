// Run: node --experimental-strip-types --test test/parked-workspace-keys.test.mjs
//
// Workspaces stay mounted when another is open: parked, invisible, `inert`.
// `inert` keeps the pointer and the focus out -- but a key listener on
// `window` hears every key whichever workspace is open. Found 2026-11-26:
//
//   - in the Drawing workspace, Delete (which removes the selected VIEW)
//     also ran the part scene's Delete: every part still selected in the
//     parked scene was deleted from the model; and the feature card's
//     Delete removed the feature still selected in the parked tree;
//   - in the Part workspace, Delete on a part also removed the view left
//     selected on the parked sheet.
//
// The rule: a key acts in the workspace that is open.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { WORKSPACES, partWorkspaceParked, sceneShortcutApplies } from '../src/lib/workspaces.ts'

const here = dirname(fileURLToPath(import.meta.url))
const read = (...p) => readFileSync(join(here, '..', 'src', ...p), 'utf8').replace(/\r\n/g, '\n')

test('the Part workspace is parked under Drawing and Assembly only', () => {
  assert.deepEqual(
    WORKSPACES.map((w) => [w.id, partWorkspaceParked(w.id)]),
    [
      ['part', false],
      ['assembly', true],
      ['drawing', true],
      // DFM and Simulate are checks ON the part: its viewport is the one shown.
      ['dfm', false],
      ['simulate', false],
    ],
  )
})

test("from the Drawing workspace no key reaches the part scene's shortcuts but undo and redo", () => {
  for (const key of ['Delete', 'Backspace', 'Escape', 'a', 'k', 'g', 'r', 'v', 'x', 'f', '1', '2', '3', '4']) {
    assert.equal(sceneShortcutApplies('drawing', key, false), false, key)
  }
  // Ctrl+A would select every part, unseen; the next Delete took them all.
  assert.equal(sceneShortcutApplies('drawing', 'a', true), false)
  // The history is the document's: the sheet says "Ctrl+Z undoes it".
  assert.equal(sceneShortcutApplies('drawing', 'z', true), true)
  assert.equal(sceneShortcutApplies('drawing', 'Z', true), true, 'Ctrl+Shift+Z')
  assert.equal(sceneShortcutApplies('drawing', 'y', true), true)
  assert.equal(sceneShortcutApplies('drawing', 'z', false), false)
})

test('where the scene is on screen its keys are untouched', () => {
  for (const id of ['part', 'assembly', 'dfm', 'simulate']) {
    for (const [key, ctrl] of [['Delete', false], ['Escape', false], ['a', true], ['z', true], ['g', false]]) {
      assert.equal(sceneShortcutApplies(id, key, ctrl), true, `${id} ${key}`)
    }
  }
})

test("the scene's shortcuts ask before any of them acts", () => {
  const src = read('lib', 'shortcuts.ts')
  const gate = src.indexOf(
    "if (!sceneShortcutApplies(useDocModeStore.getState().mode ?? 'part', e.key, ctrl)) return",
  )
  assert.ok(gate > 0, 'the gate is there')
  assert.ok(gate < src.indexOf('switch (e.key.toLowerCase())'), 'before the keys are dispatched')
  assert.ok(gate < src.indexOf('useSketchBench.getState().sketchId'), 'and before the sketch bench is consulted')
  // The command palette is the app's, not the scene's: it stays ahead of the gate.
  assert.ok(src.indexOf('useCommandPaletteStore.getState().toggle()') < gate)
})

test('the Part workspace is parked by the same rule its key listeners ask', () => {
  assert.ok(/const sheetWorkspace = partWorkspaceParked\(docMode\)/.test(read('App.tsx')))
})

test("a parked Part workspace's key listeners are silent", () => {
  const parked = "const parked = useDocModeStore((s) => partWorkspaceParked(s.mode ?? 'part'))"
  // Delete on the feature card deletes the selected feature.
  const card = read('components', 'panels', 'FeatureEditPanel.tsx')
  assert.ok(card.includes(parked))
  assert.ok(/const ownsKey =\s*!parked && !sketching && chapter !== null/.test(card))
  // Backspace takes a point off the sketch; Enter finishes it.
  const sketch = read('components', 'panels', 'SketchPanel.tsx')
  assert.ok(sketch.includes(parked))
  assert.ok(/if \(!sketch\.active \|\| parked\) return\s*const onKey/.test(sketch))
  // Enter applies the open fillet / chamfer / shell.
  const modify = read('components', 'panels', 'ModifyDialog.tsx')
  assert.ok(modify.includes(parked))
  assert.ok(/if \(!open \|\| parked\) return\s*const onKey/.test(modify))
})

test("a parked Drawing workspace's Delete does not remove the selected view", () => {
  const ws = read('components', 'panels', 'DrawingsWorkspace.tsx')
  const s = ws.indexOf('// Delete removes the selected view')
  assert.ok(s > 0)
  const effect = ws.slice(s, ws.indexOf('}, [docMode, selectedViewId, dimEdit, handleRemoveView])', s))
  assert.ok(/useEffect\(\(\) => \{\s*if \(docMode !== 'drawing'\) return\s*const onKey/.test(effect))
  assert.ok(effect.includes('void handleRemoveView(id)'), 'this is the listener that removes the view')
})
