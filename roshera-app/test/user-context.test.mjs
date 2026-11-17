// Run: node --experimental-strip-types --test test/user-context.test.mjs
//
// The live user context (lib/user-context.ts): what a report says, when one
// is sent, what the status bar says the agent sees -- and, against the
// source itself, that the publisher is mounted in the app, the chip is in
// the status bar, and the AgentLooked frame reaches the store.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  ContextPublisher,
  MAX_SELECTION,
  agentSeesLabel,
  buildReport,
  isLooking,
  lookLabel,
  sameReport,
} from '../src/lib/user-context.ts'

const src = (rel) => readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'src', rel), 'utf8')

const PART = 'aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa'
const base = (over = {}) => ({
  sharing: true,
  workspace: 'part',
  tool: 'select',
  selectionMode: 'object',
  selectedIds: [],
  subElements: [],
  hoveredId: null,
  cursor: null,
  faceIdsOf: () => [3, 3, 7, 7],
  feature: null,
  historyStep: null,
  camera: null,
  ...over,
})

// ── what a report says ───────────────────────────────────────────────

test('the face under the cursor is reported by its kernel id, with the point', () => {
  const r = buildReport(base({ cursor: { objectId: PART, point: [1.234567, 2, 3], faceId: 7 } }), 1000)
  assert.deepEqual(r.hover, { object_id: PART, kind: 'face', id: 7, point: [1.23, 2, 3] })
  assert.equal(r.pick_filter, 'part')
  assert.equal(r.at_ms, 1000)
})

test('a mesh with no face map reports the part, never a triangle index as a face', () => {
  const r = buildReport(base({ cursor: { objectId: PART, point: [0, 0, 0], faceId: undefined } }), 0)
  assert.deepEqual(r.hover, { object_id: PART, kind: 'part', point: [0, 0, 0] })
})

test('while picking edges the hover is an edge without an id: the kernel has not named it yet', () => {
  const r = buildReport(base({ selectionMode: 'edge', cursor: { objectId: PART, point: [0, 0, 0], faceId: 7 } }), 0)
  assert.deepEqual(r.hover, { object_id: PART, kind: 'edge', point: [0, 0, 0] })
  assert.equal(r.pick_filter, 'edge')
})

test('a tree-row hover reports the part', () => {
  assert.deepEqual(buildReport(base({ hoveredId: PART }), 0).hover, { object_id: PART, kind: 'part' })
})

test('a selected face carries its id only when the mesh names that face', () => {
  const r = buildReport(
    base({
      selectedIds: [PART],
      subElements: [
        { objectId: PART, type: 'face', index: 7 },
        { objectId: PART, type: 'face', index: 99 }, // a triangle index, not a face of this mesh
      ],
    }),
    0,
  )
  assert.deepEqual(r.selection, [
    { object_id: PART, kind: 'face', id: 7 },
    { object_id: PART, kind: 'face' },
  ])
})

test('an edge or vertex carries its id only once the kernel answered the pick', () => {
  const r = buildReport(
    base({
      subElements: [
        { objectId: PART, type: 'edge', index: 12 }, // optimistic: a triangle index
        { objectId: PART, type: 'edge', index: 40, resolved: true },
        { objectId: PART, type: 'vertex', index: 5, resolved: true },
      ],
    }),
    0,
  )
  assert.deepEqual(r.selection, [
    { object_id: PART, kind: 'edge' },
    { object_id: PART, kind: 'edge', id: 40 },
    { object_id: PART, kind: 'vertex', id: 5 },
  ])
})

test('a part selected through a pick on it is listed once, as the pick', () => {
  const other = 'bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb'
  const r = buildReport(
    base({ selectedIds: [PART, other], subElements: [{ objectId: PART, type: 'face', index: 3 }] }),
    0,
  )
  assert.deepEqual(r.selection, [
    { object_id: PART, kind: 'face', id: 3 },
    { object_id: other, kind: 'part' },
  ])
})

test('a selection past the limit is cut to it and the true count is said', () => {
  const ids = Array.from({ length: MAX_SELECTION + 44 }, (_, i) => `p${i}`)
  const r = buildReport(base({ selectedIds: ids }), 0)
  assert.equal(r.selection.length, MAX_SELECTION)
  assert.equal(r.selection_total, MAX_SELECTION + 44)
  assert.equal(buildReport(base({ selectedIds: ['a'] }), 0).selection_total, undefined)
})

test('the feature, the history step, the tool and the camera ride along', () => {
  const r = buildReport(
    base({
      feature: 7,
      historyStep: 4,
      tool: 'translate',
      camera: { position: [10.004, 0, 0], direction: [-1, 0, 0.00049], up: [0, 0, 1], projection: 'orthographic', preset: 'right' },
    }),
    0,
  )
  assert.equal(r.feature, 7)
  assert.equal(r.history_step, 4)
  assert.equal(r.tool, 'translate')
  assert.deepEqual(r.camera, { position: [10, 0, 0], direction: [-1, 0, 0], up: [0, 0, 1], projection: 'orthographic', preset: 'right' })
})

test('paused: the report is the pause and nothing else', () => {
  const r = buildReport(
    base({ sharing: false, selectedIds: [PART], cursor: { objectId: PART, point: [1, 2, 3], faceId: 7 }, feature: 7 }),
    5,
  )
  assert.deepEqual(r, { sharing: false, at_ms: 5 })
})

test('two reports are the same when only their clocks differ', () => {
  const a = buildReport(base({ hoveredId: PART }), 1)
  const b = buildReport(base({ hoveredId: PART }), 2)
  assert.equal(sameReport(a, b), true)
  assert.equal(sameReport(a, buildReport(base(), 1)), false)
  assert.equal(sameReport(null, a), false)
})

// ── when a report is sent ────────────────────────────────────────────

/** A clock and timers the test moves by hand, and a `send` it settles. */
function rig(options = {}) {
  let now = 0
  let nextId = 1
  const timers = new Map()
  const sent = []
  const acks = []
  let settle = []
  const pub = new ContextPublisher({
    now: () => now,
    schedule: (fn, ms) => {
      const id = nextId++
      timers.set(id, { at: now + ms, fn })
      return id
    },
    cancel: (id) => timers.delete(id),
    send: (report) =>
      new Promise((resolve) => {
        sent.push(report)
        settle.push(resolve)
      }),
    onAck: (ack) => acks.push(ack),
    ...options,
  })
  const advance = (ms) => {
    const until = now + ms
    for (;;) {
      const due = [...timers.entries()].filter(([, t]) => t.at <= until).sort((a, b) => a[1].at - b[1].at)[0]
      if (!due) break
      now = due[1].at
      timers.delete(due[0])
      due[1].fn()
    }
    now = until
  }
  /** Answer every request in flight (null = it could not be delivered). */
  const answer = async (ack = { seq: sent.length, agent_look: null }) => {
    const waiting = settle
    settle = []
    for (const r of waiting) r(ack)
    await new Promise((r) => setImmediate(r))
  }
  return { pub, sent, acks, advance, answer, at: () => now }
}

const hoverFace = (id, t = 0) => buildReport(base({ cursor: { objectId: PART, point: [0, 0, 0], faceId: id } }), t)

test('a change is sent once the context has been still, not on every move', () => {
  const { pub, sent, advance } = rig()
  pub.offer(hoverFace(1))
  advance(100)
  pub.offer(hoverFace(2))
  advance(100)
  pub.offer(hoverFace(3))
  assert.equal(sent.length, 0, 'still moving: nothing sent')
  advance(150)
  assert.equal(sent.length, 1)
  assert.equal(sent[0].hover.id, 3, 'the latest, not the first')
})

test('a context that never settles is still sent within the longest wait', () => {
  const { pub, sent, advance } = rig()
  for (let i = 0; i < 20; i++) {
    pub.offer(hoverFace(i))
    advance(100)
    if (sent.length > 0) break
  }
  assert.equal(sent.length, 1)
  assert.ok(sent[0].hover.id <= 10, `sent by one second, was at face ${sent[0].hover.id}`)
})

test('one request at a time: what changed in flight goes next, the rest are dropped', async () => {
  const { pub, sent, advance, answer } = rig()
  pub.offer(hoverFace(1))
  advance(150)
  assert.equal(sent.length, 1)
  pub.offer(hoverFace(2))
  advance(150)
  pub.offer(hoverFace(3))
  advance(150)
  assert.equal(sent.length, 1, 'the first is still in flight')
  await answer()
  advance(150)
  assert.equal(sent.length, 2)
  assert.equal(sent[1].hover.id, 3)
})

test('an unchanged context is not re-sent, except as a heartbeat', async () => {
  const { pub, sent, advance, answer } = rig()
  pub.offer(hoverFace(1))
  advance(150)
  await answer()
  pub.offer(hoverFace(1, 99))
  advance(500)
  assert.equal(sent.length, 1, 'same context, different clock: nothing owed')
  pub.heartbeat(hoverFace(1))
  assert.equal(sent.length, 1, 'too soon for a heartbeat')
  advance(1500)
  pub.heartbeat(hoverFace(1))
  assert.equal(sent.length, 2, 'two seconds on: the app says it is still here')
})

test('a paused app sends the pause once and no heartbeats', async () => {
  const { pub, sent, advance, answer } = rig()
  const paused = buildReport(base({ sharing: false }), 0)
  pub.offer(paused)
  advance(150)
  await answer()
  assert.deepEqual(sent.map((r) => r.sharing), [false])
  advance(10_000)
  pub.heartbeat(paused)
  assert.equal(sent.length, 1)
})

test('a report that could not be delivered is tried again on the next beat', async () => {
  const { pub, sent, advance, answer, acks } = rig()
  pub.offer(hoverFace(1))
  advance(150)
  await answer(null)
  assert.equal(acks.length, 0)
  pub.heartbeat(hoverFace(1))
  assert.equal(sent.length, 2, 'never acknowledged, so it is owed')
  await answer({ seq: 1, agent_look: { agent: 'Claude', age_ms: 300, seq: 1 } })
  assert.equal(acks[0].agent_look.agent, 'Claude')
})

test('the camera moving alone waits for the heartbeat', async () => {
  const { pub, sent, advance, answer } = rig()
  const cam = (x) => buildReport(base({ camera: { position: [x, 0, 0], direction: [-1, 0, 0], up: [0, 0, 1], projection: 'perspective' } }), 0)
  pub.offer(cam(10))
  advance(150)
  await answer()
  pub.offer(cam(20))
  advance(1000)
  assert.equal(sent.length, 1, 'orbiting does not send')
  advance(1000)
  pub.heartbeat(cam(30))
  assert.equal(sent.length, 2)
  assert.deepEqual(sent[1].camera.position, [30, 0, 0], 'the heartbeat carries the camera as it is now')
})

test('a stopped publisher sends nothing more', () => {
  const { pub, sent, advance } = rig()
  pub.offer(hoverFace(1))
  pub.stop()
  advance(5000)
  pub.heartbeat(hoverFace(2))
  assert.equal(sent.length, 0)
})

// ── what the status bar says ─────────────────────────────────────────

test('the chip names what the agent can see, in the words of the status bar', () => {
  const name = (id) => (id === PART ? 'Flange' : null)
  assert.equal(agentSeesLabel(null, name), 'not shared yet')
  assert.equal(agentSeesLabel({ sharing: false }, name), 'nothing · paused')
  assert.equal(agentSeesLabel(hoverFace(56), name), 'Flange · face 56')
  assert.equal(agentSeesLabel({ sharing: true, hover: { object_id: PART, kind: 'edge' } }, name), 'Flange · an edge')
  assert.equal(agentSeesLabel({ sharing: true, hover: { object_id: 'x', kind: 'part' } }, name), 'part')
  assert.equal(
    agentSeesLabel({ sharing: true, selection: [{ object_id: PART, kind: 'face', id: 3 }] }, name),
    'selected Flange · face 3',
  )
  assert.equal(agentSeesLabel({ sharing: true, selection: [{ object_id: PART, kind: 'part' }, { object_id: 'x', kind: 'part' }] }, name), '2 selected')
  assert.equal(agentSeesLabel({ sharing: true, selection: [], selection_total: 300 }, name), '300 selected')
  assert.equal(agentSeesLabel({ sharing: true, selection: [], feature: 7 }, name), 'feature at step 7')
  assert.equal(agentSeesLabel({ sharing: true, selection: [] }, name), 'nothing pointed at')
})

test("an agent's read is shown as looking for four seconds, then as how long ago", () => {
  const look = { agent: 'Claude', atMs: 10_000, seq: 3 }
  assert.equal(isLooking(look, 10_500), true)
  assert.equal(isLooking(look, 14_000), false)
  assert.equal(isLooking(null, 0), false)
  assert.equal(lookLabel(null, 0), null)
  assert.equal(lookLabel(look, 10_900), 'Claude looked just now')
  assert.equal(lookLabel(look, 22_000), 'Claude looked 12 s ago')
  assert.equal(lookLabel(look, 10_000 + 180_000), 'Claude looked 3 min ago')
})

// ── the production call sites ────────────────────────────────────────

test('the publisher is mounted in the app and posts to the user-context route', () => {
  assert.match(src('App.tsx'), /import \{ useUserContextPublisher \} from '@\/hooks\/use-user-context-publisher'/)
  assert.match(src('App.tsx'), /^\s*useUserContextPublisher\(route === 'workspace'\)/m, 'called by App, for the workspace')
  const hook = src('hooks/use-user-context-publisher.ts')
  assert.match(hook, /\/api\/agent\/user-context/)
  assert.match(hook, /new ContextPublisher\(/)
  assert.match(hook, /buildReport\(/)
})

test('the status bar shows what the agent sees', () => {
  assert.match(src('components/layout/StatusBar.tsx'), /<AgentSeesChip \/>/)
  const chip = src('components/layout/AgentSeesChip.tsx')
  assert.match(chip, /agentSeesLabel\(/)
  assert.match(chip, /setSharing\(/, 'the chip is also the pause')
})

test('the AgentLooked frame is in the schema and reaches the store', () => {
  assert.match(src('lib/ws-schemas.ts'), /z\.literal\('AgentLooked'\)/)
  assert.match(src('lib/ws-bridge.ts'), /case 'AgentLooked':[\s\S]{0,400}useUserContextStore\.getState\(\)\.agentLooked\(/)
})

test("the kernel's answer to an edge or vertex pick marks the selection as the kernel's id", () => {
  assert.match(src('lib/ws-bridge.ts'), /case 'SubElementResult':[\s\S]{0,700}resolved: true/)
})

test('a click is not reported to the agent while sharing is paused', () => {
  assert.match(src('components/viewport/CADMesh.tsx'), /function reportPointer\([\s\S]{0,300}if \(!useUserContextStore\.getState\(\)\.sharing\) return/)
})
