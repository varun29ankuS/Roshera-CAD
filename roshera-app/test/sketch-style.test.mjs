// Run: node --experimental-strip-types --test test/sketch-style.test.mjs
//
// THE SKETCH LOOK, ONE SOURCE (Varun 2026-11-27: the concept sketches are
// "the most aesthetic sketches you have ever done for me" -- and wanted in
// the main viewport too). lib/sketch-style.ts holds the pencil shading --
// three tones, hatching in the darker two, ink, the paper tone, the light,
// the stepped print -- and BOTH the concept cards' ray-marched shader and
// the viewport's Sketch display mode draw with it, so the look cannot drift.
// These pin the rules, the GLSL generated from them, and that neither
// renderer keeps a private copy.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const S = await import('../src/lib/sketch-style.ts')
const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', ...p.split('/')), 'utf8')

test('three tones: light (no hatch) above 0.62, one hatch above 0.3, crossed below', () => {
  assert.equal(S.sketchTone(0.9), 0)
  assert.equal(S.sketchTone(0.62), 1, 'the boundary falls into the middle tone, as in the shader (lam > 0.62)')
  assert.equal(S.sketchTone(0.5), 1)
  assert.equal(S.sketchTone(0.3), 2)
  assert.equal(S.sketchTone(0), 2)
})

test('hatching is a screen-space line pattern: one pixel of every `spacing`, along its slope', () => {
  // step(mod(x*slope + y, spacing), 1.0): ink when mod(...) <= 1.
  assert.equal(S.sketchHatch(0.5, 0.5, 5, 1), 1)
  assert.equal(S.sketchHatch(2.5, 0.5, 5, 1), 0)
  assert.equal(S.sketchHatch(5.5, 0.5, 5, 1), 1, 'repeats every spacing')
  // Opposite slope crosses it: the same pixel can be ink in one and paper in the other.
  assert.notEqual(S.sketchHatch(0.5, 1.5, 5, 1), S.sketchHatch(0.5, 1.5, 5, -1))
})

test('hatch ink per tone: none in the light, 0.55 in the middle and dark tones', () => {
  assert.equal(S.sketchShadeInk(0.9, 0.5, 0.5), 0)
  assert.equal(S.sketchShadeInk(0.5, 0.5, 0.5), 0.55)
  assert.equal(S.sketchShadeInk(0.5, 2.5, 0.5), 0, 'between the middle tone lines is paper')
})

test('the paper tone is a whisper of ink on the background; ink and warn pass through', () => {
  const p = S.sketchPalette([0.1, 0.1, 0.1], [1, 1, 1], [0.8, 0.5, 0.1])
  for (let i = 0; i < 3; i++) assert.ok(Math.abs(p.face[i] - (1 + (0.1 - 1) * S.SKETCH_PAPER_INK)) < 1e-12)
  assert.deepEqual(p.ink, [0.1, 0.1, 0.1])
  assert.deepEqual(p.warn, [0.8, 0.5, 0.1])
  // Dark theme: light ink on a dark ground -- the face is still mostly ground.
  const d = S.sketchPalette([0.9, 0.9, 0.9], [0.05, 0.05, 0.05], [0.8, 0.6, 0.2])
  assert.ok(d.face[0] < 0.2, `dark paper ${d.face[0]}`)
})

test('the light comes from the viewer, up and to the left, in every renderer', () => {
  const l = S.sketchLightView()
  assert.ok(Math.abs(Math.hypot(...l) - 1) < 1e-12)
  assert.ok(l[0] < 0 && l[1] > 0 && l[2] > 0, JSON.stringify(l))
})

test('the print is mechanical: fixed steps, fixed interval, instant under reduced motion', () => {
  assert.deepEqual(S.REVEAL_STEPS, [0.25, 0.5, 0.75, 1])
  assert.equal(S.revealStart(false), 0)
  assert.equal(S.revealStart(true), 1, 'reduced motion: the whole drawing at once')
  assert.equal(S.nextReveal(0), 0.25)
  assert.equal(S.nextReveal(0.5), 0.75)
  assert.equal(S.nextReveal(1), 1)
  assert.ok(S.REVEAL_STEP_MS > 0 && S.REVEAL_STEP_MS <= 100)
})

test('the GLSL library is generated from the same constants', () => {
  const g = S.SKETCH_STYLE_GLSL
  for (const fn of ['sketchHatch', 'sketchTone', 'sketchShadeInk', 'sketchCrease', 'sketchUnrevealed', 'sketchLightDir'])
    assert.match(g, new RegExp(`\\b${fn}\\(`), fn)
  assert.ok(g.includes('0.62') && g.includes('0.3') && g.includes('0.55'), 'tone thresholds and hatch strength')
})

test('neither renderer keeps a private copy of the look', () => {
  const concept = src('lib/concept-glsl.ts')
  assert.ok(concept.includes('SKETCH_STYLE_GLSL'), 'the concept shader embeds the shared library')
  assert.doesNotMatch(concept, /float hatch\(/, 'no private hatch in the concept shader')
  assert.doesNotMatch(concept, /lam > 0\.62/, 'no private tone rule in the concept shader')
  const viewport = src('lib/sketch-material.ts')
  assert.ok(viewport.includes('SKETCH_STYLE_GLSL'), 'the viewport material embeds the shared library')
  const renderer = src('lib/concept-renderer.ts')
  assert.doesNotMatch(renderer, /mix\(bg\.r, fg\.r/, 'colours come from sketch-style, not a copy')
})
