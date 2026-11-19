// Run: node --experimental-strip-types --test test/drawing-watch.test.mjs
//
// Varun 2026-11-19: "when the agent starts to make drawing, shouldn't the
// tab change to drawing?" It changed only when the agent's whole turn ended
// -- a minute or more after the sheet existed, while the agent was still
// tolerancing and fixing it. The app now watches the drawing list during a
// turn and opens a new sheet the moment it appears (the agent's tool calls
// never reach the app over ACP, so the list is the signal).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { watchForNewDrawing } from '../src/lib/drawing-watch.ts'

const here = dirname(fileURLToPath(import.meta.url))
const tick = () => new Promise((r) => setTimeout(r, 0))

test('a sheet made mid-turn is opened at once, and only once', async () => {
  let ids = ['old']
  const opened = []
  let fire = null
  const stop = watchForNewDrawing(new Set(['old']), {
    list: async () => ids,
    open: (id) => opened.push(id),
    every: (fn) => {
      fire = fn
      return () => {
        fire = null
      }
    },
  })
  await fire()
  assert.deepEqual(opened, [], 'nothing new yet')
  ids = ['old', 'sheet-1']
  await fire()
  assert.deepEqual(opened, ['sheet-1'], 'opened as soon as it exists')
  await fire()
  assert.deepEqual(opened, ['sheet-1'], 'not re-opened on the next poll')
  stop()
  assert.equal(fire, null, 'stopping cancels the poll')
  await tick()
})

test('a failing list never throws into the turn', async () => {
  let fire = null
  watchForNewDrawing(new Set(), {
    list: async () => {
      throw new Error('offline')
    },
    open: () => assert.fail('nothing to open'),
    every: (fn) => {
      fire = fn
      return () => {}
    },
  })
  await fire()
})

test('the agent turn starts the watch and stops it in its finally', () => {
  const src = readFileSync(join(here, '..', 'src', 'lib', 'acp-blackboard.ts'), 'utf8')
  assert.ok(src.includes('watchForNewDrawing('), 'the turn watches for a new sheet')
  assert.ok(/finally\s*\{[\s\S]*?stopDrawingWatch\(\)/.test(src), 'and stops watching when it ends')
})
