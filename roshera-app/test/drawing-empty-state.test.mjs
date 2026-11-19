// Run: node --experimental-strip-types --test test/drawing-empty-state.test.mjs
//
// Found rehearsing the demo video (2026-11-18): the empty Drawing workspace
// only said "Open the menu at the top left and start one from a part" -- the
// first thing an engineer does there, hidden behind a menu. With parts in the
// document, the empty state offers "Make a drawing of <part>" for each, the
// same action the menu's "New drawing from a part" takes.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const src = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')

test('an empty Drawing workspace offers a drawing of each part, one click', () => {
  const empty = src.slice(src.indexOf('drawings.length === 0'), src.indexOf('<DrawingSwitcher'))
  assert.ok(/parts\.map\(/.test(empty), 'one button per part')
  assert.ok(/Make a drawing of/.test(empty), 'said as the action')
  assert.ok(/handleCreateFromPart\(/.test(empty), 'the same action the menu takes')
  assert.ok(/No parts yet/.test(empty), 'with no parts, it says what to do first')
})
