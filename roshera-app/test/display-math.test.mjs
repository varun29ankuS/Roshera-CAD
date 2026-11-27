// Run: node --experimental-strip-types --test test/display-math.test.mjs
//
// An equation the agent writes on a line of its own (`$$m = \rho V$$`) is a
// displayed equation, not a word in a sentence. remark-math reads a one-line
// `$$…$$` as INLINE math, so it rendered as a short paragraph of text with no
// space around it and blended into the prose (Varun 2026-11-27: "the gap
// between equations and text has to be managed better").
// `promoteDisplayMath` rewrites such a line to the fenced display form, never
// inside a code fence, and never when the equation shares its line with text.
import { test } from 'node:test'
import assert from 'node:assert/strict'

const { promoteDisplayMath } = await import('../src/lib/math-delimiters.ts')

test('an equation alone on its line becomes a displayed equation', () => {
  assert.equal(
    promoteDisplayMath('Mass\n$$m=\\rho V$$\nmore text'),
    'Mass\n\n$$\nm=\\rho V\n$$\n\nmore text',
  )
})

test('leading and trailing spaces around the line do not matter', () => {
  assert.equal(promoteDisplayMath('  $$a+b$$  '), '\n$$\na+b\n$$\n')
})

test('an equation inside a sentence stays inline', () => {
  const s = 'the area is $$A = 2$$ square units'
  assert.equal(promoteDisplayMath(s), s)
  const t = 'side area is $403.23 - 20.16$ mm'
  assert.equal(promoteDisplayMath(t), t)
})

test('two equations on one line are not one display block', () => {
  const s = '$$a$$ and $$b$$'
  assert.equal(promoteDisplayMath(s), s)
})

test('nothing inside a code fence is touched', () => {
  const s = '```roshera:part\n$$x$$\n```'
  assert.equal(promoteDisplayMath(s), s)
})

test('an already fenced display block is left as it is', () => {
  const s = '$$\nx = 1\n$$'
  assert.equal(promoteDisplayMath(s), s)
})
