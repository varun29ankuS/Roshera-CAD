// Run: node --experimental-strip-types --test test/selection-attachment.test.mjs
//
// The person's selection travels with their message
// (lib/selection-attachment.ts): which chips the composer offers, what the
// agent is sent (a fenced `roshera:selection` block carrying kernel ids),
// how a sent line is read back into its words and its chips after a reload,
// and that the block is never read as kernel evidence. Against the source
// itself: the chips are mounted in the composer, the serializer is on the
// send path, and the user's line renders the chips instead of the block.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  MAX_ATTACHED,
  SELECTION_FENCE,
  attachSelection,
  attachmentOf,
  chipText,
  composerChips,
  entityKey,
  messageText,
  resolutionsFromContext,
  serializeSelection,
  splitSelection,
} from '../src/lib/selection-attachment.ts'
import { evidenceByLastLine, turnEvidence } from '../src/lib/turn-evidence.ts'
import { classifyBlackboardContent } from '../src/lib/blackboard-content.ts'
import { cardsInText, lineVerdict } from '../src/lib/blackboard-line-state.ts'

const src = (rel) => readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'src', rel), 'utf8')

const FLANGE = 'aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa'
const BRACKET = 'bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb'
const names = new Map([
  [FLANGE, 'Flange'],
  [BRACKET, 'Bracket'],
])
const solids = new Map([
  [FLANGE, 7],
  [BRACKET, 9],
])

const report = (selection, over = {}) => ({ sharing: true, tool: 'select', selection, at_ms: 1, ...over })

const input = (over = {}) => ({
  sharing: true,
  report: report([
    { object_id: FLANGE, kind: 'face', id: 56 },
    { object_id: FLANGE, kind: 'edge', id: 112 },
  ]),
  nameOf: (id) => names.get(id) ?? null,
  solidIdOf: (id) => solids.get(id),
  resolved: new Map(),
  ...over,
})

// What GET /api/agent/user-context answers (user_context.rs resolve_entity).
const resolvedBody = {
  state: 'live',
  selection: [
    {
      kind: 'face',
      object_id: FLANGE,
      id: 56,
      solid_id: 7,
      part_name: 'solid_7',
      part_persistent_id: '0f0e0d0c',
      persistent_id: 'a1b2c3d4',
      surface_type: 'plane',
      area: 1256.637061,
      resolved: true,
    },
    {
      kind: 'edge',
      object_id: FLANGE,
      id: 112,
      solid_id: 7,
      part_persistent_id: '0f0e0d0c',
      persistent_id: 'e5f6a7b8',
      curve_kind: 'circle',
      length: 125.66370614,
      resolved: true,
    },
    {
      kind: 'face',
      object_id: BRACKET,
      id: 3,
      solid_id: 9,
      resolved: false,
      reason: 'face 3 is not in the model: a later operation replaced it',
      surface_type: 'plane',
      area: 10,
    },
    { kind: 'vertex', object_id: BRACKET, id: 4, solid_id: 9, persistent_id: null, position: [1, 2.5, -3], resolved: true },
  ],
}

// ── Which chips the composer offers ──────────────────────────────────

test('nothing selected attaches nothing, and the message is sent byte-identical', () => {
  const { chips, more } = composerChips(input({ report: report([]) }))
  assert.deepEqual(chips, [])
  assert.equal(more, 0)
  const att = attachmentOf(chips, more, new Set())
  assert.equal(serializeSelection(att), '')
  assert.equal(attachSelection('fillet this edge 2 mm', att), 'fillet this edge 2 mm')
  // No report at all yet (the server never acknowledged one): same.
  assert.deepEqual(composerChips(input({ report: null })).chips, [])
})

test('paused sharing attaches nothing, even before the server acknowledged the pause', () => {
  // The store says paused; the last acknowledged report still lists a face.
  assert.deepEqual(composerChips(input({ sharing: false })).chips, [])
  // And an acknowledged pause carries nothing whatever the local flag says.
  assert.deepEqual(composerChips(input({ report: report([{ object_id: FLANGE, kind: 'face', id: 1 }], { sharing: false }) })).chips, [])
})

test('chips name the part the person sees, the kind and the kernel id, in selection order', () => {
  const { chips } = composerChips(input())
  assert.deepEqual(
    chips.map((c) => chipText(c.item).title),
    ['Flange · face 56', 'Flange · edge 112'],
  )
  assert.equal(chips[0].item.solid_id, 7, 'the kernel solid id an agent tool takes, from the scene')
  assert.equal(chips[0].key, entityKey({ object_id: FLANGE, kind: 'face', id: 56 }))
})

test('an edge the kernel has not answered for is attached without an id and says so', () => {
  const { chips } = composerChips(input({ report: report([{ object_id: FLANGE, kind: 'edge' }]) }))
  assert.equal(chipText(chips[0].item).title, 'Flange · an edge')
  assert.equal(chips[0].item.id, undefined)
})

test('kernel resolution adds persistent ids and the kernel measure; unresolved entities get neither', () => {
  const resolved = resolutionsFromContext(resolvedBody)
  const { chips } = composerChips(
    input({
      report: report([
        { object_id: FLANGE, kind: 'face', id: 56 },
        { object_id: FLANGE, kind: 'edge', id: 112 },
        { object_id: BRACKET, kind: 'face', id: 3 },
        { object_id: BRACKET, kind: 'vertex', id: 4 },
      ]),
      resolved,
    }),
  )
  const [face, edge, stale, vertex] = chips.map((c) => c.item)
  assert.equal(face.pid, 'a1b2c3d4')
  assert.equal(face.part_pid, '0f0e0d0c')
  assert.equal(face.surface, 'plane')
  assert.equal(face.area_mm2, 1256.637)
  assert.equal(chipText(face).measure, 'plane · 1256.64 mm²')
  assert.equal(edge.pid, 'e5f6a7b8')
  assert.equal(chipText(edge).measure, 'circle · 125.66 mm')
  // A replaced face: the part is still known, the face's numbers are not.
  assert.equal(stale.solid_id, 9)
  assert.equal(stale.pid, undefined)
  assert.equal(stale.area_mm2, undefined)
  assert.equal(chipText(stale).measure, '')
  assert.deepEqual(vertex.at_mm, [1, 2.5, -3])
  assert.equal(vertex.pid, undefined, 'the kernel assigns a vertex no persistent id: none is invented')
  assert.equal(chipText(vertex).measure, '(1, 2.5, −3) mm')
})

test("the scene's part name wins over the kernel's; the kernel's fills in when the scene has none", () => {
  const resolved = resolutionsFromContext(resolvedBody)
  assert.equal(composerChips(input({ resolved })).chips[0].item.part, 'Flange')
  const anon = composerChips(input({ resolved, nameOf: () => null })).chips[0].item
  assert.equal(anon.part, 'solid_7')
})

test('a part chip carries the kernel size when it was read', () => {
  const resolved = new Map([[entityKey({ object_id: BRACKET, kind: 'part' }), { size_mm: [80, 40.5, 12] }]])
  const { chips } = composerChips(input({ report: report([{ object_id: BRACKET, kind: 'part' }]), resolved }))
  assert.equal(chipText(chips[0].item).title, 'Bracket')
  assert.equal(chipText(chips[0].item).measure, '80 × 40.5 × 12 mm')
})

test('a large selection attaches the first MAX_ATTACHED and counts the rest, using selection_total', () => {
  const many = Array.from({ length: 30 }, (_, i) => ({ object_id: FLANGE, kind: 'face', id: i + 1 }))
  const { chips, more } = composerChips(input({ report: report(many, { selection_total: 300 }) }))
  assert.equal(chips.length, MAX_ATTACHED)
  assert.equal(more, 300 - MAX_ATTACHED)
  const plain = composerChips(input({ report: report(many) }))
  assert.equal(plain.more, 30 - MAX_ATTACHED)
})

test('a removed chip is not attached; removing "more" drops the count', () => {
  const many = Array.from({ length: MAX_ATTACHED + 2 }, (_, i) => ({ object_id: FLANGE, kind: 'face', id: i + 1 }))
  const { chips, more } = composerChips(input({ report: report(many) }))
  const removed = new Set([chips[0].key])
  const att = attachmentOf(chips, more, removed)
  assert.equal(att.items.length, MAX_ATTACHED - 1)
  assert.ok(!att.items.some((i) => i.id === 1))
  assert.equal(att.more, 2)
  assert.equal(attachmentOf(chips, more, new Set(['more'])).more, 0)
  // Every chip removed: nothing is attached at all.
  const all = new Set([...chips.map((c) => c.key), 'more'])
  assert.equal(attachSelection('hi', attachmentOf(chips, more, all)), 'hi')
})

// ── What the agent is sent ───────────────────────────────────────────

test('the block: a fenced roshera:selection, one JSON object per entity, fixed key order', () => {
  const resolved = resolutionsFromContext(resolvedBody)
  const { chips, more } = composerChips(input({ resolved }))
  const block = serializeSelection(attachmentOf(chips, more, new Set()))
  assert.equal(SELECTION_FENCE, 'roshera:selection')
  assert.equal(
    block,
    [
      '```roshera:selection',
      '{"kind":"face","part":"Flange","object_id":"aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa","solid_id":7,"id":56,"pid":"a1b2c3d4","part_pid":"0f0e0d0c","surface":"plane","area_mm2":1256.637}',
      '{"kind":"edge","part":"Flange","object_id":"aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa","solid_id":7,"id":112,"pid":"e5f6a7b8","part_pid":"0f0e0d0c","curve":"circle","length_mm":125.664}',
      '```',
    ].join('\n'),
  )
})

test('the block is deterministic: the same selection always serializes to the same bytes', () => {
  const resolved = resolutionsFromContext(resolvedBody)
  const a = serializeSelection(attachmentOf(...Object.values(composerChips(input({ resolved }))), new Set()))
  // Same facts, resolution map built in the opposite order.
  const reversed = resolutionsFromContext({ ...resolvedBody, selection: [...resolvedBody.selection].reverse() })
  const b = serializeSelection(attachmentOf(...Object.values(composerChips(input({ resolved: reversed }))), new Set()))
  assert.equal(a, b)
})

test('the count of entities not listed rides as a last {"more":n} line', () => {
  const many = Array.from({ length: MAX_ATTACHED + 3 }, (_, i) => ({ object_id: FLANGE, kind: 'face', id: i + 1 }))
  const { chips, more } = composerChips(input({ report: report(many) }))
  const lines = serializeSelection(attachmentOf(chips, more, new Set())).split('\n')
  assert.equal(lines.at(-2), '{"more":3}')
})

test('a part name with backticks cannot close the fence early', () => {
  const { chips, more } = composerChips(input({ nameOf: () => 'evil ``` name' }))
  const text = attachSelection('look', attachmentOf(chips, more, new Set()))
  assert.equal(text.split('```').length - 1, 2, 'exactly the opening and closing fence')
  assert.equal(splitSelection(text).attachment.items[0].part, 'evil ``` name')
})

test('the message: the words, a blank line, the block — and splitSelection reads it back exactly', () => {
  const resolved = resolutionsFromContext(resolvedBody)
  const { chips, more } = composerChips(input({ resolved }))
  const att = attachmentOf(chips, more, new Set())
  const text = attachSelection('fillet this edge 2 mm', att)
  assert.ok(text.startsWith('fillet this edge 2 mm\n\n```roshera:selection\n'))
  const back = splitSelection(text)
  assert.equal(back.prose, 'fillet this edge 2 mm')
  assert.deepEqual(back.attachment, att)
  assert.equal(messageText(text), 'fillet this edge 2 mm')
  // A store or transport that adds a trailing newline does not lose the chips.
  assert.deepEqual(splitSelection(`${text}\n`).attachment, att)
  assert.deepEqual(splitSelection(`${text}\r\n`).attachment, att)
})

test('text without a block, or with a malformed one, is returned whole and unread', () => {
  assert.deepEqual(splitSelection('just words'), { prose: 'just words', attachment: null })
  const bad = 'x\n\n```roshera:selection\n{"kind":"face","part":"P"}\n```' // no object_id
  assert.deepEqual(splitSelection(bad), { prose: bad, attachment: null })
  const notJson = 'x\n\n```roshera:selection\nface 56\n```'
  assert.equal(splitSelection(notJson).attachment, null)
  // A block that is not at the end of the message is not the attachment.
  const middle = 'x\n\n```roshera:selection\n{"kind":"part","part":"P","object_id":"o"}\n```\nmore words'
  assert.equal(splitSelection(middle).attachment, null)
  assert.equal(messageText(middle), middle)
  // `more` anywhere but last, or not a positive integer: malformed.
  const moreFirst = 'x\n\n```roshera:selection\n{"more":2}\n{"kind":"part","part":"P","object_id":"o"}\n```'
  assert.equal(splitSelection(moreFirst).attachment, null)
})

// ── Never mistaken for evidence ──────────────────────────────────────

const blockText = () => {
  const resolved = resolutionsFromContext(resolvedBody)
  const { chips, more } = composerChips(input({ resolved }))
  return attachSelection('fillet this edge 2 mm', attachmentOf(chips, more, new Set()))
}

test('the selection block is not a card, not evidence, and gives no turn a verdict', () => {
  const text = blockText()
  const block = serializeSelection(splitSelection(text).attachment)
  assert.deepEqual(cardsInText(text), [])
  assert.equal(lineVerdict(text), null)
  const user = { id: 'u1', text, author: 'user', timestamp: 1 }
  // The agent quoting the block back, alone or inside prose.
  const echo = { id: 'a1', text: block, author: 'agent', timestamp: 2 }
  const prose = { id: 'a2', text: `Filleting the edge you selected:\n\n${block}\n\nDone.`, author: 'agent', timestamp: 3 }
  assert.equal(classifyBlackboardContent(user), 'reasoning', 'the user line keeps its own box, not the evidence lane')
  assert.equal(classifyBlackboardContent(echo), 'reasoning')
  assert.equal(classifyBlackboardContent(prose), 'reasoning')
  const turns = turnEvidence([user, echo, prose])
  assert.equal(turns.length, 1)
  assert.deepEqual(turns[0].marks, [])
  assert.equal(turns[0].verdict, null)
  assert.equal(evidenceByLastLine([user, echo, prose]).size, 0)
})

// ── Wired into the app ───────────────────────────────────────────────

test('the composer mounts the selection chips inside its form', () => {
  const board = src('components/panels/Blackboard.tsx')
  const form = board.slice(board.indexOf('<form'), board.indexOf('</form>'))
  assert.ok(form.length > 0, 'the composer form exists')
  assert.match(form, /<ComposerSelectionChips\b/)
  assert.match(board, /useSelectionChips\(/)
})

test('the serializer is on the send path: handleSubmit attaches before sendPrompt', () => {
  const board = src('components/panels/Blackboard.tsx')
  const submit = board.slice(board.indexOf('const handleSubmit'), board.indexOf('const handleKeyDown'))
  const attach = submit.indexOf('attachSelection(')
  const send = submit.indexOf('sendPrompt(')
  assert.ok(attach >= 0, 'handleSubmit calls attachSelection')
  assert.ok(send > attach, 'and sends what it produced')
  // History keeps the words, never the block.
  assert.ok(submit.indexOf('pushPromptHistory(historyRef.current, text)') >= 0)
  assert.match(submit, /sendPrompt\(withSelection,/)
})

test("the user's line renders its chips, read back from the stored text", () => {
  const line = src('components/panels/BlackboardLine.tsx')
  assert.match(line, /splitSelection\(line\.text\)/)
  assert.match(line, /<LineSelectionChips\b/)
})

test('the queue strip and the collapsed stub show the words, not the block', () => {
  const board = src('components/panels/Blackboard.tsx')
  assert.equal(board.split('messageText(w.text)').length - 1, 2, 'the chip text and its tooltip')
  assert.match(board, /messageText\(lines\[lines\.length - 1\]/)
})
