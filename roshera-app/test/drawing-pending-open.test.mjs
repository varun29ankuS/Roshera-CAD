// Run: node --experimental-strip-types --test test/drawing-pending-open.test.mjs
//
// Found rehearsing the aerospace video (2026-11-19): the agent made the
// wing rib's drawing and switched to the Drawing workspace, which still said
// "No drawings yet". `openDrawing(id)` records the drawing as pending, but
// the workspace consumed the pending id only on its FIRST mount -- and
// workspaces stay mounted when parked, so every later drawing was ignored.
// The workspace now opens a pending drawing whenever one arrives.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')

test('a drawing opened while the workspace is mounted is listed and shown', () => {
  assert.ok(
    /const pendingDrawingId = useDocModeStore\(\(s\) => s\.pendingDrawingId\)/.test(ws),
    'the workspace watches the pending drawing',
  )
  const at = ws.indexOf('[pendingDrawingId]')
  assert.ok(at > 0, 'an effect runs when a pending drawing arrives')
  const effect = ws.slice(ws.lastIndexOf('useEffect(', at), at)
  assert.ok(effect.includes('consumePendingDrawing()'), 'it takes the pending id')
  assert.ok(/refreshList\(/.test(effect), 'and reloads the list landing on it')
})
