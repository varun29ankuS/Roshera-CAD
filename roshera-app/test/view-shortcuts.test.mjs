import { test } from 'node:test'
import assert from 'node:assert/strict'

const { viewPresetForKey, VIEW_SHORTCUT_LABEL } = await import('../src/lib/view-shortcuts.ts')

const key = (code, mods = {}) => ({ code, shiftKey: false, ctrlKey: false, metaKey: false, altKey: false, ...mods })

test('Shift+1..7 are the standard views, in the order engineers know from Onshape', () => {
  const views = ['Digit1', 'Digit2', 'Digit3', 'Digit4', 'Digit5', 'Digit6', 'Digit7'].map((c) =>
    viewPresetForKey(key(c, { shiftKey: true })),
  )
  assert.deepEqual(views, ['front', 'back', 'left', 'right', 'top', 'bottom', 'isometric'])
})

test('a bare digit is NOT a view: 1..4 belong to the pick modes', () => {
  for (const c of ['Digit1', 'Digit2', 'Digit3', 'Digit4', 'Digit7']) {
    assert.equal(viewPresetForKey(key(c)), null, c)
  }
})

test('the numpad drives the views, with Ctrl for the opposite side', () => {
  assert.equal(viewPresetForKey(key('Numpad1')), 'front')
  assert.equal(viewPresetForKey(key('Numpad1', { ctrlKey: true })), 'back')
  assert.equal(viewPresetForKey(key('Numpad3')), 'right')
  assert.equal(viewPresetForKey(key('Numpad3', { ctrlKey: true })), 'left')
  assert.equal(viewPresetForKey(key('Numpad7')), 'top')
  assert.equal(viewPresetForKey(key('Numpad7', { ctrlKey: true })), 'bottom')
  assert.equal(viewPresetForKey(key('Numpad0')), 'isometric')
})

test('browser and OS chords are left alone', () => {
  assert.equal(viewPresetForKey(key('Digit1', { shiftKey: true, ctrlKey: true })), null)
  assert.equal(viewPresetForKey(key('Digit1', { shiftKey: true, altKey: true })), null)
  assert.equal(viewPresetForKey(key('Digit1', { shiftKey: true, metaKey: true })), null)
  assert.equal(viewPresetForKey(key('KeyA', { shiftKey: true })), null)
})

test('every view a menu can print a shortcut for is reachable by that shortcut', () => {
  const codeFor = { '1': 'Digit1', '2': 'Digit2', '3': 'Digit3', '4': 'Digit4', '5': 'Digit5', '6': 'Digit6', '7': 'Digit7' }
  for (const [view, label] of Object.entries(VIEW_SHORTCUT_LABEL)) {
    const m = /^Shift\+(\d)$/.exec(label)
    assert.ok(m, `${view}: "${label}" is a Shift+digit label`)
    assert.equal(viewPresetForKey(key(codeFor[m[1]], { shiftKey: true })), view)
  }
})
