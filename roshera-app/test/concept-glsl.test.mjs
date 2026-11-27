// Run: node --experimental-strip-types --test test/concept-glsl.test.mjs
//
// The shader the concept sketch ray-marches is GENERATED from the recipe
// (lib/concept-glsl.ts) out of the same prepared primitives the TS field
// evaluates (lib/concept-sdf.ts `prepareRecipe`), so the picture and the
// numbers come from one description. These pin the generator: one call per
// step in order, the right combine per op, GLSL-legal float literals.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { BRACKET_SET } from './concept-fixtures.mjs'

const { parseCard } = await import('../src/lib/blackboard-cards.ts')
const { glslFloat, sceneGlsl, GLSL_SDF_LIBRARY } = await import('../src/lib/concept-glsl.ts')

const set = (() => {
  const r = parseCard('concept', JSON.stringify(BRACKET_SET))
  assert.equal(r.ok, true)
  return r.card.card
})()

test('float literals always carry a decimal point (GLSL ES has no implicit int->float)', () => {
  assert.equal(glslFloat(3), '3.0')
  assert.equal(glslFloat(-0.25), '-0.25')
  assert.equal(glslFloat(1e-7), '0.0000001')
  assert.throws(() => glslFloat(Number.NaN))
})

test('one primitive call per step, combined in order by its op', () => {
  const a = sceneGlsl(set.concepts[0].recipe)
  assert.equal((a.match(/sdBox\(/g) ?? []).length, 2)
  assert.equal((a.match(/sdRib\(/g) ?? []).length, 2)
  assert.equal((a.match(/sdCylinder\(/g) ?? []).length, 1)
  assert.equal((a.match(/opUnion\(/g) ?? []).length, 3)
  assert.equal((a.match(/opSubtract\(/g) ?? []).length, 1)
  assert.ok(a.indexOf('sdRib(') < a.indexOf('sdCylinder('), 'the bore is cut after the ribs are added')
  const c = sceneGlsl(set.concepts[2].recipe)
  assert.match(c, /opUnionRound\(d, s, 3\.0\)/, 'the blend radius reaches the shader')
  const b = sceneGlsl(set.concepts[1].recipe)
  assert.match(b, /sdPlate\(/)
  assert.match(b, /sdSlot\(/)
})

test('the library defines every function the generator calls', () => {
  for (const fn of ['sdBox', 'sdPlate', 'sdCylinder', 'sdCone', 'sdRib', 'sdSlot', 'opUnion', 'opUnionRound', 'opSubtract', 'opIntersect']) {
    assert.match(GLSL_SDF_LIBRARY, new RegExp(`float ${fn}\\(`), fn)
  }
  assert.match(sceneGlsl(set.concepts[0].recipe), /^float sceneSdf\(vec3 p\) \{/)
})
