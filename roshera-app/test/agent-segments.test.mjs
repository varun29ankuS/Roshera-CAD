import { test } from 'node:test'
import assert from 'node:assert/strict'

const { joinAgentText, needsNewSegment } = await import('../src/lib/agent-segments.ts')

test("the agent's messages are paragraphs, not one run-on line", () => {
  // The three messages from the refused-fillet turn, as they streamed.
  let t = ''
  for (const m of [
    "I'll start by looking at what's in the model.",
    "I'm finding the inside corner edge on the bracket so I can fillet it.",
    'The fillet was refused, so the bracket is unchanged.',
  ])
    t = joinAgentText(t, m)
  assert.equal(
    t,
    "I'll start by looking at what's in the model.\n\nI'm finding the inside corner edge on the bracket so I can fillet it.\n\nThe fillet was refused, so the bracket is unchanged.",
  )
})

test('chunks inside a sentence or a word join as they came', () => {
  assert.equal(joinAgentText('The fil', 'let was refused'), 'The fillet was refused')
  assert.equal(joinAgentText('a 5 mm', ' fillet'), 'a 5 mm fillet')
  assert.equal(joinAgentText('R5.', '0 mm'), 'R5.0 mm', 'a number after a point is not a new sentence')
  assert.equal(joinAgentText('', 'Hello'), 'Hello')
})

test('a line added after the one the agent is writing moves its next words below it', () => {
  assert.equal(needsNewSegment(['u1', 'a1'], 'a1'), false)
  assert.equal(needsNewSegment(['u1', 'a1', 'intent'], 'a1'), true)
  assert.equal(needsNewSegment([], 'a1'), false)
})
