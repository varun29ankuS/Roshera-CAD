// Run: node --experimental-strip-types --test test/toolbar-import.test.mjs
//
// Varun 2026-11-19: "what about importing a file ... I see export, I don't see
// import" / "I only see export in make toolbar". Import STEP lived as the
// second section inside the flyout labelled "Export". It is now its own
// button in MAKE, before Export, opening the same file picker.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const src = readFileSync(join(here, '..', 'src', 'components', 'layout', 'ToolBar.tsx'), 'utf8')

test('Import is its own MAKE button, before Export, and opens the STEP picker', () => {
  const make = src.slice(src.indexOf("label: 'MAKE'"), src.indexOf("flyout(exportGroup, 'Export')") + 40)
  const imp = make.indexOf("label: 'Import'")
  assert.ok(imp > 0, 'an Import button in MAKE')
  assert.ok(imp < make.indexOf("flyout(exportGroup"), 'before Export')
  assert.ok(make.slice(imp - 200, imp + 300).includes('roshera:open-step-import'), 'it opens the STEP picker')
})

test('the Export flyout no longer hides Import', () => {
  const group = src.slice(src.indexOf('const exportGroup'), src.indexOf('const tool ='))
  assert.ok(!group.includes("label: 'Import'"), 'Import is not buried in Export')
})
