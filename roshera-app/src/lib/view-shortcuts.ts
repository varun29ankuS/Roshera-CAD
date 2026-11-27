/**
 * The keys that turn the model to a standard view.
 *
 * Shift+1..7 -- Front, Back, Left, Right, Top, Bottom, Isometric -- the
 * order browser CAD users already know (Onshape). The bare digits 1..4 are
 * the pick modes (Part / Face / Edge / Vertex), and Ctrl+digit belongs to the
 * browser's tabs and never reaches a page, so Shift is the modifier a web
 * app can actually own.
 *
 * The numpad works too, for people who came from tools that use it:
 * 1 Front, 3 Right, 7 Top, 0 Isometric, with Ctrl for the opposite side.
 *
 * Matching is on `code` (the physical key), not `key`: with Shift held a
 * digit's `key` is a punctuation mark that depends on the keyboard layout,
 * and a numpad key's `key` is the same digit as the top row's.
 */

export type StandardView = 'front' | 'back' | 'left' | 'right' | 'top' | 'bottom' | 'isometric'

/** The part of a KeyboardEvent the mapping reads. */
export interface ViewKey {
  code: string
  shiftKey: boolean
  ctrlKey: boolean
  metaKey: boolean
  altKey: boolean
}

const SHIFT_DIGIT: Readonly<Record<string, StandardView>> = {
  Digit1: 'front',
  Digit2: 'back',
  Digit3: 'left',
  Digit4: 'right',
  Digit5: 'top',
  Digit6: 'bottom',
  Digit7: 'isometric',
}

const NUMPAD: Readonly<Record<string, readonly [plain: StandardView, withCtrl: StandardView]>> = {
  Numpad1: ['front', 'back'],
  Numpad3: ['right', 'left'],
  Numpad7: ['top', 'bottom'],
  Numpad0: ['isometric', 'isometric'],
}

/** What a menu prints beside each view. Each one is a key that works: the
 *  test for this module presses every label and checks the view it gives. */
export const VIEW_SHORTCUT_LABEL: Readonly<Record<StandardView, string>> = {
  front: 'Shift+1',
  back: 'Shift+2',
  left: 'Shift+3',
  right: 'Shift+4',
  top: 'Shift+5',
  bottom: 'Shift+6',
  isometric: 'Shift+7',
}

/** The view a key press asks for, or null when it is not a view key. */
export function viewPresetForKey(e: ViewKey): StandardView | null {
  if (e.altKey || e.metaKey) return null
  const pad = NUMPAD[e.code]
  if (pad) return e.ctrlKey ? pad[1] : pad[0]
  if (e.shiftKey && !e.ctrlKey) return SHIFT_DIGIT[e.code] ?? null
  return null
}
