// Run: node --experimental-strip-types --test test/turn-evidence.test.mjs
//
// The turn evidence strip (lib/turn-evidence.ts): every agent turn ends in
// the kernel's answer -- one tick / cross / slash per certificate the turn
// carried, worst verdict wins, and a turn that built geometry without a
// soundness certificate on the board says so. Plus tail-follow scrolling
// (lib/blackboard-scroll.ts) and, against the source itself, that both are
// mounted where the conversation renders.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { evidenceByLastLine, turnEvidence } from '../src/lib/turn-evidence.ts'
import { isAtTail } from '../src/lib/blackboard-scroll.ts'

const src = (rel) => readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'src', rel), 'utf8')

const FENCE = '```'
const fence = (kind, payload) => `${FENCE}roshera:${kind}\n${JSON.stringify(payload, null, 2)}\n${FENCE}`

// verify_part output shape (as in blackboard-cards.test.mjs, trimmed to the
// fields the schema needs).
const sound = (ok) => ({ part_id: 12, sound: ok, brep_valid: ok, brep_watertight: ok, manifold: ok })
const dfmRule = (kind) => ({
  rule: 'fdm.min_wall',
  verdict:
    kind === 'pass'
      ? { kind: 'pass', margin: { value: 0.4, derivation: { kind: 'analytic', surface_type: 'plane', method: 'x' } } }
      : kind === 'violation'
        ? {
            kind: 'violation',
            witnesses: [3],
            measured: { value: 0.2, derivation: { kind: 'analytic', surface_type: 'plane', method: 'x' } },
            limit: { value: 0.8, derivation: { kind: 'analytic', surface_type: 'plane', method: 'x' } },
          }
        : { kind: 'unverifiable', regions: [3], reason: { kind: 'unsupported_topology', detail: 'd' } },
  provenance: { kind: 'shop_practice', note: 'n' },
})
const refusal = { reason: 'fillet radius exceeds the adjacent face curvature', error_code: 'BLEND_ENVELOPE' }

let n = 0
const line = (author, text) => ({ id: `l${++n}`, author, text, createdAt: 1000 + n, updatedAt: 1000 + n })
const ask = (t) => line('user', t)
const agent = (t) => line('agent', t)
const echo = (name) => line('system', `Created **${name}** — 18 × 18 × 20 mm · 792 tris`)

const words = (ev) => ev.marks.map((m) => `${m.verdict}:${m.word}`)

test('a certified turn: one tick per certificate, in order, verdict pass', () => {
  const lines = [ask('make a box'), echo('box 1'), agent(`Built.\n\n${fence('soundness', sound(true))}`)]
  const [turn] = turnEvidence(lines)
  assert.deepEqual(words(turn), ['pass:sound'])
  assert.equal(turn.verdict, 'pass')
  assert.equal(turn.lastLineId, lines[2].id)
})

test('worst verdict wins: a later failure is never hidden by an earlier pass', () => {
  const lines = [
    ask('bores'),
    agent(fence('soundness', sound(true))),
    agent(fence('dfm', dfmRule('violation'))),
    agent(fence('dfm', dfmRule('unverifiable'))),
  ]
  const [turn] = turnEvidence(lines)
  assert.deepEqual(words(turn), ['pass:sound', 'fail:DFM violation', 'inconclusive:DFM inconclusive'])
  assert.equal(turn.verdict, 'fail')
})

test('can-not-say beats pass, and is never a pass', () => {
  const lines = [ask('fillet'), agent(fence('soundness', sound(true))), agent(fence('refusal', refusal))]
  const [turn] = turnEvidence(lines)
  assert.deepEqual(words(turn), ['pass:sound', 'inconclusive:refused'])
  assert.equal(turn.verdict, 'inconclusive')
})

test('an unsound certificate is a cross', () => {
  const [turn] = turnEvidence([ask('x'), agent(fence('soundness', sound(false)))])
  assert.deepEqual(words(turn), ['fail:unsound'])
  assert.equal(turn.verdict, 'fail')
})

test('geometry built with no soundness certificate on the board says so', () => {
  const lines = [ask('bore it'), echo('bore 1/4'), echo('bore 2/4'), agent('Done, four bores.')]
  const [turn] = turnEvidence(lines)
  assert.deepEqual(words(turn), ['inconclusive:no certificate shown'])
  assert.equal(turn.verdict, 'inconclusive')
})

test('a turn still streaming is not yet called uncertified; its real marks show live', () => {
  const reply = agent('Boring the flange…')
  const lines = [ask('bores'), echo('bore 1/4'), echo('bore 2/4'), reply]
  assert.equal(evidenceByLastLine(lines, reply.id).size, 0)
  const settled = evidenceByLastLine(lines, null).get(reply.id)
  assert.deepEqual(words(settled), ['inconclusive:no certificate shown'])
  // A certificate that already arrived mid-turn is shown at once.
  const withCert = [...lines.slice(0, 3), agent(fence('soundness', sound(true))), reply]
  assert.deepEqual(words(evidenceByLastLine(withCert, reply.id).get(reply.id)), ['pass:sound'])
})

test('a DFM pass alone does not certify built geometry', () => {
  const lines = [ask('x'), echo('box 1'), agent(fence('dfm', dfmRule('pass')))]
  const [turn] = turnEvidence(lines)
  assert.deepEqual(words(turn), ['pass:DFM pass', 'inconclusive:no certificate shown'])
})

test('a WS-level "Server refused:" line is a refusal mark', () => {
  const [turn] = turnEvidence([ask('x'), line('system', 'Server refused: no such face 99')])
  assert.deepEqual(words(turn), ['inconclusive:refused'])
})

test('a user typing "Server refused:" or a fence is content, not evidence', () => {
  const lines = [ask(`Server refused: why?\n\n${fence('soundness', sound(true))}`)]
  assert.equal(evidenceByLastLine(lines).size, 0)
})

test('a prose-only turn has nothing to certify and gets no strip', () => {
  const lines = [ask('what is a datum?'), agent('A datum is a reference.')]
  assert.equal(evidenceByLastLine(lines).size, 0)
})

test('turns split only at user lines; adjacent asks share the run that follows them', () => {
  const lines = [
    ask('a'),
    agent(fence('soundness', sound(true))),
    ask('b'),
    ask('c'),
    echo('box 2'),
    agent(fence('soundness', sound(false))),
  ]
  const turns = turnEvidence(lines)
  assert.equal(turns.length, 2)
  assert.deepEqual(words(turns[0]), ['pass:sound'])
  assert.deepEqual(words(turns[1]), ['fail:unsound'])
  const byLast = evidenceByLastLine(lines)
  assert.deepEqual([...byLast.keys()], [lines[1].id, lines[5].id])
})

test('a choices card and an FCF with no verdict carry no mark', () => {
  const choices = `${FENCE}roshera:choices\nquestion: "Fit?"\noptions:\n  - H7/g6\n  - H7/h6\n${FENCE}`
  const lines = [ask('x'), agent(choices)]
  assert.equal(evidenceByLastLine(lines).size, 0)
})

test('marks point at the line that carries the card', () => {
  const lines = [ask('x'), agent('working'), agent(fence('soundness', sound(true)))]
  const [turn] = turnEvidence(lines)
  assert.equal(turn.marks[0].lineId, lines[2].id)
})

// ── tail-follow scrolling ─────────────────────────────────────────────

test('the board follows the tail only while the reader is at it', () => {
  assert.equal(isAtTail({ scrollTop: 600, scrollHeight: 1000, clientHeight: 400 }), true)
  assert.equal(isAtTail({ scrollTop: 560, scrollHeight: 1000, clientHeight: 400 }), true)
  assert.equal(isAtTail({ scrollTop: 300, scrollHeight: 1000, clientHeight: 400 }), false)
  // Content shorter than the viewport: always at the tail.
  assert.equal(isAtTail({ scrollTop: 0, scrollHeight: 200, clientHeight: 400 }), true)
})

// ── production call sites ─────────────────────────────────────────────

test('the evidence strip is computed for the whole notebook and mounted after each turn', () => {
  const board = src('components/panels/Blackboard.tsx')
  assert.match(board, /evidenceByLastLine\(lines, streamingLineId\)/)
  assert.match(board, /evidence=\{evidence\}/)
  const section = src('components/panels/BlackboardSection.tsx')
  assert.match(section, /<TurnEvidenceStrip\b/)
  assert.match(section, /evidence\.get\(/)
})

test('a mark can find its line: every line carries its id in the DOM', () => {
  assert.match(src('components/panels/BlackboardLine.tsx'), /data-line-id=\{line\.id\}/)
  // ...and a collapsed build strip carries the ids of the lines it folds,
  // which the strip's reveal falls back to.
  assert.match(src('components/panels/BuildStepStrip.tsx'), /data-line-ids=\{lines\.map\(\(l\) => l\.id\)\.join\(' '\)\}/)
  assert.match(src('components/panels/TurnEvidenceStrip.tsx'), /\[data-line-ids~=/)
})

test('autoscroll is gated on the reader being at the tail, with a way back', () => {
  const board = src('components/panels/Blackboard.tsx')
  assert.match(board, /onScroll=\{/)
  assert.match(board, /isAtTail\(/)
  assert.match(board, /Jump to latest/)
  // The autoscroll effect itself must be the gated one, not a second,
  // unconditional scroll beside it.
  assert.match(board, /if \(el && followTailRef\.current\) el\.scrollTop = el\.scrollHeight/)
  assert.doesNotMatch(board, /useEffect\(\(\) => \{\s*if \(scrollRef\.current\) \{\s*scrollRef\.current\.scrollTop = scrollRef\.current\.scrollHeight/)
})

test('turn ended is not certified: the completed glyph is not emerald', () => {
  const lineSrc = src('components/panels/BlackboardLine.tsx')
  const glyph = lineSrc.slice(lineSrc.indexOf('function TurnStatusGlyph'), lineSrc.indexOf('function PartTag'))
  assert.doesNotMatch(glyph, /status === 'completed' && 'text-emerald/)
})

test('no conversation text below the 11 px floor', () => {
  for (const f of ['components/panels/Blackboard.tsx', 'components/panels/TurnEvidenceStrip.tsx']) {
    assert.doesNotMatch(src(f), /text-\[(?:[0-9]|10)(?:\.\d+)?px\]/, f)
  }
})
