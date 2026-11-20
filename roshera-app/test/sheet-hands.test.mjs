// Run: node --experimental-strip-types --test test/sheet-hands.test.mjs
//
// Agent eyes, slice 3 (2026-11-20): when the agent's hands change a sheet
// (POST /api/drawings/{id}/layout), the server sends an AgentShow `sheet_act`
// frame (each moved thing's old and new box, the rule, the caption) and then
// a `sheet_judge` frame (each touched space's score change, the grade, the
// verdict). The app glides each box old -> new over ~500 ms with a fading
// ghost at the old place for ~1 s, then flashes the judged spaces green or
// red, worst first. Reduced motion: no glide, no fade, no stagger.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { serverMessageSchema } from '../src/lib/ws-schemas.ts'
import { showCaption } from '../src/lib/agent-show.ts'
import {
  ACT_GLIDE_MS,
  actJudgeCaption,
  judgeDelayMs,
  GHOST_MS,
  GHOST_OPACITY,
  JUDGE_FLASH_MS,
  JUDGE_GREEN,
  JUDGE_RED,
  JUDGE_STAGGER_MS,
  actRunning,
  flashOpacity,
  ghostOpacity,
  judgeFlashes,
  judgeRunning,
  lerpRect,
  movedBoxAt,
  scoreVerdict,
} from '../src/lib/sheet-hands.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (...p) => readFileSync(join(here, '..', 'src', ...p), 'utf8')
const DRAWING = 'aaaaaaaa-1111-4222-8333-444444444444'
const frame = (payload) => ({ type: 'AgentShow', payload })

/** The act frame exactly as the layout route's serde writes it. */
const ACT = {
  action: 'sheet_act',
  drawing_id: DRAWING,
  verb: 'reattach_leader',
  moves: [
    {
      kind: 'dimension',
      id: 'd27',
      from: { x0: 459.07, y0: 76.6, x1: 532.16, y1: 96.6 },
      to: { x0: 440.2, y0: 70.1, x1: 459.6, y1: 81.5 },
    },
  ],
  rule: 'LEADER-ANGLE',
  caption:
    "re-routed the leader of '28.5°' on FRONT: 62 mm -> 20 mm at 55 degrees -- fixes LEADER-ANGLE: the leader of '28.5°' meets its shoulder at 14.1 degrees",
}
const JUDGE = {
  action: 'sheet_judge',
  drawing_id: DRAWING,
  spaces: [
    { space: 'view:FRONT/band:top', before: 1.0, after: 0.85, rect: { x0: 1, y0: 2, x1: 30, y1: 12 } },
    { space: 'view:FRONT/band:right', before: 0.7225, after: 1.0, rect: { x0: 40, y0: 2, x1: 60, y1: 50 } },
    { space: 'view:RIGHT', before: 0.85, after: null, rect: { x0: 100, y0: 2, x1: 160, y1: 50 } },
    { space: 'view:FRONT/geometry', before: 0.85, after: 0.85, rect: null },
  ],
  grade_before: 0.321,
  grade_after: 0.425,
  below_bar_before: 7,
  below_bar_after: 6,
  sound: true,
  complete: true,
  caption: 'grade 0.32 -> 0.43, below bar 7 -> 6, SOUND, complete',
}

test('the act and judge frames parse as the server writes them', () => {
  for (const f of [ACT, JUDGE]) {
    const r = serverMessageSchema.safeParse(frame(f))
    assert.ok(r.success, JSON.stringify(r.error?.issues))
    assert.deepEqual(r.data.payload, f)
  }
  // A removed view: an old box and no new one.
  const removed = { ...ACT, verb: 'remove_view', moves: [{ kind: 'view', id: 'RIGHT', from: ACT.moves[0].from, to: null }] }
  assert.ok(serverMessageSchema.safeParse(frame(removed)).success)
})

test('malformed act and judge frames are refused at the boundary', () => {
  for (const [why, payload] of [
    ['no moves', { ...ACT, moves: [] }],
    ['a move with neither box', { ...ACT, moves: [{ kind: 'dimension', id: 'd27', from: null, to: null }] }],
    ['an unknown kind', { ...ACT, moves: [{ ...ACT.moves[0], kind: 'table' }] }],
    ['an inverted box', { ...ACT, moves: [{ ...ACT.moves[0], to: { x0: 9, y0: 0, x1: 1, y1: 1 } }] }],
    ['not a drawing id', { ...ACT, drawing_id: 'sheet-7' }],
    ['a score past 1', { ...JUDGE, spaces: [{ space: 'x', before: 0.5, after: 1.5 }] }],
    ['a grade under 0', { ...JUDGE, grade_after: -0.1 }],
    ['a fractional count', { ...JUDGE, below_bar_after: 1.5 }],
  ]) {
    assert.equal(serverMessageSchema.safeParse(frame(payload)).success, false, why)
  }
})

test('the bar says the server caption, else what was done or judged', () => {
  assert.equal(showCaption(ACT), ACT.caption)
  assert.equal(showCaption({ ...ACT, caption: null }), 'reattach leader: d27 (LEADER-ANGLE)')
  assert.equal(showCaption(JUDGE), JUDGE.caption)
  assert.equal(showCaption({ ...JUDGE, caption: null, grade_after: 0.4251 }), 'grade 0.32 → 0.43, below bar 7 → 6, SOUND')
})

test('a moved box glides from its old box to its new one, eased, then rests there', () => {
  const m = ACT.moves[0]
  assert.deepEqual(movedBoxAt(m, 0, false), m.from, 'starts at the old box')
  assert.deepEqual(movedBoxAt(m, ACT_GLIDE_MS, false), m.to, 'lands on the new box')
  assert.deepEqual(movedBoxAt(m, 5 * ACT_GLIDE_MS, false), m.to, 'and stays')
  const mid = movedBoxAt(m, ACT_GLIDE_MS / 2, false)
  // Eased, symmetric: exactly half way at the half time.
  const half = lerpRect(m.from, m.to, 0.5)
  for (const k of ['x0', 'y0', 'x1', 'y1']) assert.ok(Math.abs(mid[k] - half[k]) < 1e-9, k)
  // Slow at the ends: a tenth of the time covers well under a tenth of the way.
  const early = movedBoxAt(m, ACT_GLIDE_MS / 10, false)
  assert.ok((early.x0 - m.from.x0) / (m.to.x0 - m.from.x0) < 0.05)
  assert.ok(ACT_GLIDE_MS >= 400 && ACT_GLIDE_MS <= 600, 'about 500 ms')
  // Linear interpolation itself, by this test's arithmetic.
  assert.deepEqual(lerpRect({ x0: 0, y0: 0, x1: 10, y1: 10 }, { x0: 10, y0: 20, x1: 30, y1: 40 }, 0.25), {
    x0: 2.5,
    y0: 5,
    x1: 15,
    y1: 17.5,
  })
})

test('a thing that appeared is at its new box; one that went is gone after the glide', () => {
  const appeared = { kind: 'view', id: 'X', from: null, to: { x0: 1, y0: 1, x1: 2, y1: 2 } }
  assert.deepEqual(movedBoxAt(appeared, 0, false), appeared.to)
  const went = { kind: 'view', id: 'RIGHT', from: { x0: 1, y0: 1, x1: 2, y1: 2 }, to: null }
  assert.deepEqual(movedBoxAt(went, 0, false), went.from)
  assert.equal(movedBoxAt(went, ACT_GLIDE_MS, false), null)
})

test('the ghost stays at the old box about a second, fading; never without an old box', () => {
  const m = ACT.moves[0]
  assert.equal(ghostOpacity(m, 0, false), GHOST_OPACITY)
  const half = ghostOpacity(m, GHOST_MS / 2, false)
  assert.ok(half > 0 && half < GHOST_OPACITY, `fading: ${half}`)
  assert.equal(ghostOpacity(m, GHOST_MS, false), 0, 'gone after its lifetime')
  assert.ok(GHOST_MS >= 800 && GHOST_MS <= 1200, 'about a second')
  assert.equal(ghostOpacity({ ...m, from: null }, 0, false), 0)
  assert.ok(actRunning(GHOST_MS - 1) && !actRunning(Math.max(GHOST_MS, ACT_GLIDE_MS)))
})

test('reduced motion: the new box at once, the ghost still for its lifetime, flashes together', () => {
  const m = ACT.moves[0]
  assert.deepEqual(movedBoxAt(m, 0, true), m.to, 'no glide')
  assert.equal(ghostOpacity(m, 0, true), GHOST_OPACITY, 'the ghost is shown')
  assert.equal(ghostOpacity(m, GHOST_MS / 2, true), GHOST_OPACITY, 'and does not fade')
  assert.equal(ghostOpacity(m, GHOST_MS, true), 0, 'then goes')
  const f = judgeFlashes(JUDGE.spaces, true)
  assert.ok(f.every((x) => x.delayMs === 0), 'no stagger')
  assert.ok(f.every((x) => flashOpacity(x, JUDGE_FLASH_MS / 2, true) === 1), 'steady')
})

test('the judge flashes each touched space, worst first, green better and red worse', () => {
  assert.equal(scoreVerdict(JUDGE.spaces[0]), 'worse')
  assert.equal(scoreVerdict(JUDGE.spaces[1]), 'better')
  assert.equal(scoreVerdict(JUDGE.spaces[2]), 'gone')
  assert.equal(scoreVerdict({ space: 's', before: null, after: 0.5 }), 'new')
  const f = judgeFlashes(JUDGE.spaces, false)
  assert.deepEqual(
    f.map((x) => [x.space, x.color]),
    [
      ['view:FRONT/band:top', JUDGE_RED],
      ['view:RIGHT', JUDGE_GREEN],
      ['view:FRONT/band:right', JUDGE_GREEN],
    ],
    'worst first (0.85 after, then 0.85 before for the space that went, then 1.0); a space with no box is not drawn',
  )
  assert.equal(f[0].label, '1.00 → 0.85')
  assert.deepEqual(f.map((x) => x.delayMs), [0, JUDGE_STAGGER_MS, 2 * JUDGE_STAGGER_MS], 'staggered')
  assert.equal(flashOpacity(f[1], 0, false), 0, 'not before its turn')
  assert.equal(flashOpacity(f[0], JUDGE_FLASH_MS / 2, false), 1, 'full in the middle')
  assert.equal(flashOpacity(f[0], JUDGE_FLASH_MS, false), 0, 'gone at the end')
  assert.ok(judgeRunning(f, JUDGE_FLASH_MS) && !judgeRunning(f, JUDGE_FLASH_MS + 2 * JUDGE_STAGGER_MS))
})

test('the store carries the act and the judgement to the viewer, which draws them', () => {
  const store = src('stores', 'agent-show-store.ts')
  for (const [action, field] of [
    ["case 'sheet_act'", 'sheetAct'],
    ["case 'sheet_judge'", 'sheetJudge'],
  ]) {
    const at = store.indexOf(action)
    assert.ok(at > 0, `the store handles ${action}`)
    const body = store.slice(at, store.indexOf('return', at))
    assert.ok(body.includes('openDrawing(a.drawing_id)'), `${action} opens that drawing`)
    assert.ok(new RegExp(`set\\(\\{\\s*${field}:`).test(body), `${action} hands it to the viewer`)
  }
  assert.ok(/takeOver: \(\) =>\s*set\(\{[^}]*sheetAct: null[^}]*sheetJudge: null/.test(store), 'Take over clears both')
  const ws = src('components', 'panels', 'DrawingsWorkspace.tsx')
  assert.ok(ws.includes('useAgentShowStore((s) => s.sheetAct)'), 'the viewer reads the act')
  assert.ok(ws.includes('useAgentShowStore((s) => s.sheetJudge)'), 'the viewer reads the judgement')
  assert.ok(ws.includes('movedBoxAt(m, actT, handsReduced)'), 'it glides with the pure interpolation')
  assert.ok(ws.includes('ghostOpacity(m, actT, handsReduced)'), 'it draws the ghost')
  assert.ok(ws.includes('judgeFlashes(judgeShown.spaces, handsReduced, judgeShown.afterMs)'), 'it flashes the judged spaces, after the act')
  assert.ok(ws.includes('const handsReduced = prefersReducedMotion()'), 'it honours reduced motion')
  assert.ok(/void refreshActive\(actDrawing\)/.test(ws), 'it re-reads the sheet the verb changed')
  assert.ok(ws.includes('data-act-ghost=') && ws.includes('data-judge-verdict='))
})

test('JUDGE comes after ACT: the flashes wait out the glide, and the act caption stays in front', () => {
  assert.equal(judgeDelayMs(0, false), ACT_GLIDE_MS, 'a judgement on the heels of its act waits the whole glide')
  assert.equal(judgeDelayMs(200, false), ACT_GLIDE_MS - 200)
  assert.equal(judgeDelayMs(ACT_GLIDE_MS + 10, false), 0, 'the glide is over: no wait')
  assert.equal(judgeDelayMs(0, true), 0, 'reduced motion: no wait')
  assert.equal(judgeDelayMs(null, false), 0, 'no act running: no wait')
  const f = judgeFlashes(JUDGE.spaces, false, ACT_GLIDE_MS)
  assert.ok(f.every((x) => x.delayMs >= ACT_GLIDE_MS), 'every flash after the glide')
  assert.equal(flashOpacity(f[0], ACT_GLIDE_MS - 1, false), 0, 'nothing flashes during the glide')
  assert.ok(judgeFlashes(JUDGE.spaces, true, ACT_GLIDE_MS).every((x) => x.delayMs === 0), 'reduced motion: at once')
  assert.equal(actJudgeCaption(ACT.caption, JUDGE.caption), `${ACT.caption} — ${JUDGE.caption}`)
  assert.match(actJudgeCaption(ACT.caption, JUDGE.caption), /LEADER-ANGLE/, 'the rule id stays on the bar')
  assert.equal(actJudgeCaption(null, JUDGE.caption), JUDGE.caption)
  const store = src('stores', 'agent-show-store.ts')
  const at = store.indexOf("case 'sheet_judge'")
  const body = store.slice(at, store.indexOf('return\n', at))
  assert.ok(body.includes('actJudgeCaption(act.caption, caption)'), 'the bar keeps the act caption in front')
  assert.ok(body.includes('judgeDelayMs(sinceAct, prefersReducedMotion())'), 'the judgement waits for the act')
  assert.ok(/at: Date\.now\(\)/.test(store.slice(store.indexOf("case 'sheet_act'"))), 'the act records when it landed')
})