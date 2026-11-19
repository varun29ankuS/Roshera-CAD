// Run: node --experimental-strip-types --test test/import-summary.test.mjs
//
// Varun (2026-11-19, rehearsing the Volvo video): the STEP import card was
// 11 px monospace with the raw schema string ("AUTOMOTIVE_DESIGN { 1 0 10303
// 214 1 1 1 1 }") and entity counts up front. The card now leads with a plain
// verdict and the schema by name; the entity counts stay one click away
// ("Import details"), never removed.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { schemaLabel } from '../src/lib/import-report.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

test('the STEP schema is named the way engineers say it', () => {
  assert.equal(schemaLabel('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'), 'STEP AP214 · automotive design')
  assert.equal(schemaLabel('CONFIG_CONTROL_DESIGN'), 'STEP AP203 · configuration controlled design')
  assert.equal(
    schemaLabel('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 1 1 4 }'),
    'STEP AP242 · model-based 3D engineering',
  )
})

test('an unknown schema is shown as its name, without the object id', () => {
  assert.equal(schemaLabel('SOME_OTHER_SCHEMA { 1 0 10303 999 1 }'), 'SOME_OTHER_SCHEMA')
  assert.equal(schemaLabel(null), null)
  assert.equal(schemaLabel(''), null)
})

test('the card leads with the verdict; the counts are one click away', () => {
  const card = src('components/viewport/StepImportDropzone.tsx')
  const body = card.slice(card.indexOf('function ReportBody'))
  assert.ok(body.includes('schemaLabel('), 'the schema by name')
  assert.ok(body.includes('<details'), 'the entity counts sit behind a fold')
  assert.ok(/Import details/.test(body), 'labelled plainly')
  const head = body.slice(0, body.indexOf('<details'))
  assert.ok(!head.includes('report.roots_resolved'), 'no raw counts before the fold')
  assert.ok(!/className="px-2\.5 py-2 text-\[11px\] font-mono"/.test(body), 'the card is not all monospace')
})
