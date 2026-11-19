// Run: node --experimental-strip-types --test test/card-plain-text.test.mjs
//
// Found rehearsing the aerospace video (2026-11-19): the agent's GD&T result
// arrived in a `roshera:fcf` card as a readable line ("⌖ 0.10mm A B -> IN SPEC
// ...") and the Blackboard printed "roshera:fcf payload failed validation --
// Unexpected token '⌖' ... is not valid JSON" above it. A card whose payload
// is plain text is shown as that text; only a payload that IS structured
// data and fails its schema says so.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { isStructuredPayload } from '../src/lib/card-payload.ts'

const here = dirname(fileURLToPath(import.meta.url))

test('JSON payloads are structured; a readable line is not', () => {
  assert.equal(isStructuredPayload('{"kind":"fcf"}'), true)
  assert.equal(isStructuredPayload('  [1, 2]'), true)
  assert.equal(isStructuredPayload('⌖ 0.10mm A B → IN SPEC (measured 0.00mm)'), false)
  assert.equal(isStructuredPayload(''), false)
})

test('the card shows plain text without a validation error', () => {
  const src = readFileSync(join(here, '..', 'src', 'components', 'panels', 'cards', 'CardRenderer.tsx'), 'utf8')
  const at = src.indexOf('isStructuredPayload(')
  assert.ok(at > 0, 'the renderer asks whether the payload is data')
  assert.ok(at < src.indexOf('payload failed validation'), 'before it would report a validation failure')
})
