// Run: node --experimental-strip-types --test test/import-report.test.mjs
//
// Found rehearsing the demo video (2026-11-18): importing the bracket showed
// "✓ imported clean" and, under it in red, "6306 entities failed (9 types)".
// The importer's "failed" bucket is "the handler raised a warning OR an
// error" -- mostly repairs and warnings on a part whose solid checked sound.
// Calling all of them failed, in red, under a green verdict contradicts
// itself. Said as what it is: amber when the solid is sound, red only when
// it is not; the entity types are listed on hover.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { entityIssueLine } from '../src/lib/import-report.ts'

const here = dirname(fileURLToPath(import.meta.url))

test('a sound solid: warnings are said as warnings, in amber, never "failed"', () => {
  const line = entityIssueLine({ B_SPLINE_CURVE_WITH_KNOTS: 6000, EDGE_CURVE: 306 }, true)
  assert.equal(line.text, '6306 entities raised warnings or errors (2 types) — the solid still checks sound')
  assert.equal(line.tone, 'caution')
  assert.ok(!/failed/.test(line.text))
  assert.match(line.title, /B_SPLINE_CURVE_WITH_KNOTS: 6000/)
  assert.match(line.title, /EDGE_CURVE: 306/)
})

test('an unsound solid: the same counts are a failure, in red', () => {
  const line = entityIssueLine({ ADVANCED_FACE: 1 }, false)
  assert.equal(line.text, '1 entity failed (1 type)')
  assert.equal(line.tone, 'negative')
})

test('no issues: no line', () => {
  assert.equal(entityIssueLine({}, true), null)
})

test('the import card uses it, not its own "failed" wording', () => {
  const src = readFileSync(join(here, '..', 'src', 'components', 'viewport', 'StepImportDropzone.tsx'), 'utf8')
  assert.ok(src.includes('entityIssueLine('))
  assert.ok(!/entit\{failedTotal === 1/.test(src), 'the old unconditional "failed" line is gone')
})
