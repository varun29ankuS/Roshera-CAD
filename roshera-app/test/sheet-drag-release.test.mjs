// Run: node --experimental-strip-types --test test/sheet-drag-release.test.mjs
//
// A drag on the drawing sheet (pan, a view, a dimension) is held from the
// mouse-down to the mouse-up. When the mouse-up never reaches the page --
// Alt+Tab mid-drag, a system dialog -- the drag stayed in hand: on return
// the sheet, the view's ghost or the dimension's text followed a pointer
// nobody was pressing, and the next click DROPPED it, sending a move the
// engineer never made (2026-11-26). The rule now:
//
//   - a pointer movement the browser reports with the primary button up,
//     while a drag is in hand, ends the drag;
//   - so does the window losing focus, and any new press;
//   - a drag ended that way sends nothing (where it would have been dropped
//     is not known);
//   - a scripted MouseEvent says nothing about the button (its `buttons` is
//     0 unless its author set it), so it never ends a drag: the app's own
//     checks and recordings, which synthesise mouse events, keep working.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { releaseWasMissed } from '../src/lib/sheet-drag-release.ts'

const here = dirname(fileURLToPath(import.meta.url))
const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8').replace(/\r\n/g, '\n')

test('a real movement with the primary button up means the release was missed', () => {
  assert.equal(releaseWasMissed({ isTrusted: true, buttons: 0 }), true)
  // Only another button is held (the right one, the wheel): the primary is up.
  assert.equal(releaseWasMissed({ isTrusted: true, buttons: 2 }), true)
  assert.equal(releaseWasMissed({ isTrusted: true, buttons: 4 }), true)
})

test('a movement with the primary button held is the drag going on', () => {
  assert.equal(releaseWasMissed({ isTrusted: true, buttons: 1 }), false)
  // Held together with another button.
  assert.equal(releaseWasMissed({ isTrusted: true, buttons: 3 }), false)
})

test('a scripted mouse event never ends a drag', () => {
  // `new MouseEvent('mousemove', { clientX, clientY })` -- what a scripted
  // drag dispatches -- has buttons 0 and is not trusted.
  assert.equal(releaseWasMissed({ isTrusted: false, buttons: 0 }), false)
  assert.equal(releaseWasMissed({ isTrusted: false, buttons: 1 }), false)
  assert.equal(releaseWasMissed({ isTrusted: false, buttons: 2 }), false)
})

// ── The workspace asks it, and ends every drag the same way ──────────────

const effectStart = ws.indexOf('const dragInHand = () =>')
const effectEnd = ws.indexOf('/** Open a dimension', effectStart)
const effect = ws.slice(effectStart, effectEnd)

test('every drag the workspace holds counts as in hand', () => {
  assert.ok(effectStart > 0 && effectEnd > effectStart, 'the drag listeners are found')
  const inHand = effect.slice(0, effect.indexOf('const abandonDrags'))
  for (const held of ['dimDragRef.current !== null', 'viewDragRef.current !== null', 'panStateRef.current.dragging']) {
    assert.ok(inHand.includes(held), `${held} is a drag in hand`)
  }
})

test('letting go drops all three drags and sends nothing', () => {
  const s = effect.indexOf('const abandonDrags = () => {')
  const body = effect.slice(s, effect.indexOf('const onMouseMove', s))
  for (const dropped of [
    "el.style.translate = ''",
    'dimDragRef.current = null',
    'viewDragRef.current = null',
    'setViewGhost(null)',
    'panStateRef.current.dragging = false',
    "el.style.cursor = ''",
  ]) {
    assert.ok(body.includes(dropped), `${dropped} when a drag is let go`)
  }
  assert.ok(!/commit\w*Ref\.current\(/.test(body), 'no move is sent for a release nobody saw')
})

test('a move with the button up ends the drag before it is followed', () => {
  const s = effect.indexOf('const onMouseMove = (e: MouseEvent) => {')
  const body = effect.slice(s, effect.indexOf('const onMouseUp', s))
  const guard = body.search(/if \(dragInHand\(\) && releaseWasMissed\(e\)\) \{\s*abandonDrags\(\)\s*return\s*\}/)
  assert.ok(guard > 0, 'the move handler asks whether the release was missed')
  for (const follows of ['dimDragRef.current', 'viewDragRef.current', 'setPan(']) {
    assert.ok(body.indexOf(follows) > guard, `it asks before ${follows} follows the pointer`)
  }
})

test('a new press and a lost window end a drag too; Escape goes the same way', () => {
  assert.ok(/const onPressBegins = \(\) => abandonDrags\(\)/.test(effect))
  assert.ok(/const onWindowBlur = \(\) => abandonDrags\(\)/.test(effect))
  assert.ok(
    /if \(e\.key === 'Escape' && \(dimDragRef\.current \|\| viewDragRef\.current\)\) abandonDrags\(\)/.test(effect),
    'Escape lets go through the same function',
  )
  // Capture phase: before the sheet's own mouse-down starts the next drag.
  assert.ok(effect.includes("window.addEventListener('mousedown', onPressBegins, true)"))
  assert.ok(effect.includes("window.addEventListener('blur', onWindowBlur)"))
})

test('every listener the drag effect adds is removed, and an unmount lets go', () => {
  const added = [...effect.matchAll(/window\.addEventListener\(([^)]*)\)/g)].map((m) => m[1])
  const removed = [...effect.matchAll(/window\.removeEventListener\(([^)]*)\)/g)].map((m) => m[1])
  assert.ok(added.length >= 5)
  assert.deepEqual(removed, added, 'same listeners, same phase')
  const cleanup = effect.slice(effect.indexOf('return () => {', effect.indexOf('window.addEventListener')))
  assert.ok(/^return \(\) => \{[\s\S]*?abandonDrags\(\)[\s\S]*?window\.removeEventListener/.test(cleanup))
})
