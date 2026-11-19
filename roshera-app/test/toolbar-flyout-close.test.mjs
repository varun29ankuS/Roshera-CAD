// Run: node --experimental-strip-types --test test/toolbar-flyout-close.test.mjs
//
// Found rehearsing the Volvo video (2026-11-19): the toolbar's Export flyout
// stayed open over the model tree after Escape, and stayed on screen after
// switching to the Drawing workspace (the Part toolbar is parked there, its
// flyout was not). A flyout now closes on Escape and on a workspace switch.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const toolbar = readFileSync(join(here, '..', 'src', 'components', 'layout', 'ToolBar.tsx'), 'utf8')

test('Escape closes an open flyout', () => {
  const at = toolbar.indexOf("e.key === 'Escape'")
  assert.ok(at > 0, 'the toolbar listens for Escape')
  const near = toolbar.slice(at, at + 200)
  assert.ok(near.includes('setOpenId(null)'), 'and closes the flyout')
})

test('switching workspace closes an open flyout', () => {
  assert.ok(/useDocModeStore\(\(s\) => s\.mode\)/.test(toolbar), 'the toolbar knows the workspace')
  assert.ok(/useEffect\(\(\) => \{\s*setOpenId\(null\)\s*\}, \[docMode\]\)/.test(toolbar), 'a workspace change closes it')
})
