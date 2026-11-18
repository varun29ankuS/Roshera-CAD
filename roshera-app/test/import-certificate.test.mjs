// Run: node --experimental-strip-types --test test/import-certificate.test.mjs
//
// Found rehearsing the demo video (2026-11-18): after a STEP import the
// status bar read "✓ sound" while the Blackboard marked the import "no
// certificate shown" -- the import never put the kernel's certificate on the
// board. Now it fetches the part's full certificate (the route verify_part
// uses) and posts it as a soundness card; a failed or refused check is posted
// as what it is, never as a pass.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { importCertificateLine } from '../src/lib/import-certificate.ts'

const here = dirname(fileURLToPath(import.meta.url))
const card = (line) => JSON.parse(line.match(/```roshera:soundness\n([\s\S]*?)\n```/)[1])

test('a sound import posts the kernel certificate as a soundness card', () => {
  const line = importCertificateLine('linkage-bracket', 0, {
    verdict: 'SOUND — full kernel certificate clean',
    cert: { sound: true, brep_valid: true, watertight: true, manifold: true, shells_outward: true, self_intersection_free: true, errors: [] },
  })
  assert.match(line, /^Kernel certificate for linkage-bracket/)
  const c = card(line)
  assert.equal(c.sound, true)
  assert.equal(c.part, 'linkage-bracket')
  assert.equal(c.part_id, 0)
  assert.equal(c.verdict, 'SOUND — full kernel certificate clean')
})

test('an unsound import says so', () => {
  const c = card(importCertificateLine('p', 3, { verdict: 'UNSOUND', cert: { sound: false, errors: ['open edge'] } }))
  assert.equal(c.sound, false)
  assert.deepEqual(c.errors, ['open edge'])
})

test('no certificate is never a pass: sound stays absent', () => {
  const c = card(importCertificateLine('p', 3, null))
  assert.equal(c.sound, null)
  assert.match(c.verdict, /could not be read/)
})

test('the import posts it to the Blackboard after a good import', () => {
  const src = readFileSync(join(here, '..', 'src', 'components', 'viewport', 'StepImportDropzone.tsx'), 'utf8')
  assert.ok(src.includes('importCertificateLine('), 'the line is built from the kernel answer')
  assert.ok(/perception\?full=1/.test(src), 'from the full certificate route')
  assert.ok(/addLine\(/.test(src), 'and posted to the board')
})
