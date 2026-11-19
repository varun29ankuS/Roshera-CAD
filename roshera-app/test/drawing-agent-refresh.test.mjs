// Run: node --experimental-strip-types --test test/drawing-agent-refresh.test.mjs
//
// Found rehearsing the bracket video (2026-11-19): the Drawing workspace now
// opens the agent's sheet the moment it exists, but the agent went on to set
// the bores to H7 -- and the screen kept the sheet as first loaded ("3× Ø20"
// while the server inked "3× Ø20 H7 (+0.021/0)"). When an agent turn ends,
// the open sheet is re-read.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const read = (p) => readFileSync(join(here, '..', 'src', ...p.split('/')), 'utf8')

test('an agent turn announces that it ended', () => {
  const src = read('lib/acp-blackboard.ts')
  assert.ok(/finally\s*\{[\s\S]*?roshera:agent-turn-ended/.test(src), 'the turn dispatches the event when it ends')
})

test('the Drawing workspace re-reads the open sheet when a turn ends', () => {
  const ws = read('components/panels/DrawingsWorkspace.tsx')
  const at = ws.indexOf("'roshera:agent-turn-ended'")
  assert.ok(at > 0, 'it listens for the event')
  const around = ws.slice(Math.max(0, at - 600), at + 400)
  assert.ok(/refreshActive\(/.test(around), 'and refreshes the active drawing')
})
