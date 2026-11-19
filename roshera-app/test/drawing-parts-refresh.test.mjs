// Run: node --experimental-strip-types --test test/drawing-parts-refresh.test.mjs
//
// Found rehearsing the Volvo video (2026-11-19): open the Drawing workspace,
// go back to Part, import a STEP file, return to Drawing -- it still said
// "No parts yet". Workspaces stay mounted when parked, and the Drawing
// workspace read its parts only on its first mount. It now reads them each
// time it becomes the active workspace.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')

test('entering the Drawing workspace re-reads the parts', () => {
  assert.ok(/useDocModeStore\(\(s\) => s\.mode\)/.test(ws), 'the workspace knows which workspace is active')
  assert.ok(
    /useEffect\(\(\) => \{\s*if \(docMode === 'drawing'\) void refreshParts\(\)\s*\}, \[docMode, refreshParts\]\)/.test(ws),
    'and re-reads the parts when it becomes active',
  )
})
