// Run: node --experimental-strip-types --test test/agent-policy-cards.test.mjs
//
// The Blackboard showed walls of text and no typeset math because the agent
// was never told to do otherwise: its instructions (roshera-agent-policy.md,
// compiled into the api-server and written as the goose session's AGENTS.md)
// asked for cards only when forwarding a kernel payload, invited prose
// "around the card", named no field of a refusal, and mentioned math only as
// the two words "math typeset". A renderer the agent is never taught to
// drive is wired to nothing. These pin the instructions to the renderer:
// every card kind is taught with the fields its schema requires, replies are
// card-first, and math is written as LaTeX the board typesets.
// (roshera-mcp/test/ask_choice.test.mjs pins the 'Kinds:' list to CARD_KINDS.)
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const { CARD_KINDS, parseCard } = await import('../src/lib/blackboard-cards.ts')
const here = dirname(fileURLToPath(import.meta.url))
const policy = readFileSync(join(here, '..', '..', 'roshera-agent-policy.md'), 'utf8')

test('every card kind the board renders is named in the policy', () => {
  for (const kind of CARD_KINDS) assert.match(policy, new RegExp('`' + kind + '`'), kind)
})

test('replies are card-first, with at most two short lines of prose', () => {
  assert.match(policy, /Reply card-first/)
  assert.match(policy, /at\s+most two short lines/)
  assert.doesNotMatch(policy, /Prose still carries the meaning around the card/)
})

test('the summary cards are taught with the fields their schemas require', () => {
  for (const field of ['"part"', '"dimensions_world"', '"volume"', '"mass"', '"sound"']) {
    assert.ok(policy.includes(field), `part card field ${field}`)
  }
  for (const field of ['"sheet"', '"scale"', '"projection"', '"views"', '"complete"', '"dimensions_placed"']) {
    assert.ok(policy.includes(field), `sheet card field ${field}`)
  }
  for (const field of ['"rows"', '"quantity"', '"value"', '"limit"', '"pass"']) {
    assert.ok(policy.includes(field), `measures card field ${field}`)
  }
})

test('a part summary never stands in for the certificate', () => {
  // turn-evidence.ts counts only a soundness card as a certificate; a turn
  // with a part card alone would show "part sound" AND "no certificate shown".
  assert.match(policy, /never\s+replaces\s+it/)
  assert.match(policy, /forward\s+the\s+certificate\s+itself\s+as\s+its\s+own\s+soundness\s+card/)
})

test('the refusal card is taught with its required "reason"', () => {
  assert.match(policy, /refusal: \{"reason"/)
})

test('the policy\'s own example values validate as cards', () => {
  // The part numbers the policy's mass example uses, assembled as the policy
  // describes: 312 400 mm^3 of steel at 7850 kg/m^3 is 2.45 kg.
  const r = parseCard('part', JSON.stringify({ part: 'Hub flange', volume: 312400, mass: 2.452, material: { name: 'Steel', density: 7850 } }))
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  const s = parseCard('sheet', JSON.stringify({ name: 'Hub flange', sheet: 'A3', scale: '1:2', projection: 'first' }))
  assert.equal(s.ok, true, s.ok ? '' : s.error)
})

test('math is written as LaTeX, inline and display, with engineering examples', () => {
  assert.match(policy, /Write math as math/)
  assert.ok(policy.includes('$K_t = 1 + 2b/a = 3.0$'), 'an inline stress-concentration example')
  assert.match(policy, /\r?\n\$\$\r?\n[^$]+\r?\n\$\$\r?\n/, 'a display block on its own lines')
  assert.ok(policy.includes('\\mathrm{H7/g6}'), 'an ISO 286 fit')
  assert.match(policy, /Never flatten a formula/)
})

test('the policy keeps the clauses the api-server test pins', () => {
  // goose_acp.rs roshera_agent_policy_reaches_the_system_prompt
  assert.ok(policy.startsWith('You are a mechanical design agent on the Roshera kernel'))
  assert.ok(policy.includes('kb_lookup(kind:"playbook"'))
  assert.ok(policy.includes('kb_lookup(kind:"reference"'))
  assert.doesNotMatch(policy, /@import/, 'goose would expand an @import line')
})
