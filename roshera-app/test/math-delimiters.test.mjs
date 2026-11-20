// Run: node --experimental-strip-types --test test/math-delimiters.test.mjs
//
// The Blackboard typesets `$…$` / `$$…$$` (remark-math + KaTeX), but models
// also write LaTeX's own `\(…\)` and `\[…\]` — and those reached the board as
// literal backslash-paren text. `normalizeMathDelimiters` rewrites them to the
// dollar forms before rendering, never inside code (a card fence's JSON, an
// inline code span), so the streaming splitter and the final render agree.
// `splitInlineMath` is the card-field counterpart: a note or a hint is one
// line of text with inline math in it, not a markdown document.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import ReactMarkdown from 'react-markdown'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const { normalizeMathDelimiters, splitInlineMath } = await import('../src/lib/math-delimiters.ts')
const { splitStreamingText } = await import('../src/lib/streaming-math.ts')
const { REMARK_PLUGINS, REHYPE_PLUGINS } = await import('../src/lib/markdown-pipeline.ts')

const render = (md) =>
  renderToStaticMarkup(
    createElement(
      ReactMarkdown,
      { remarkPlugins: REMARK_PLUGINS, rehypePlugins: REHYPE_PLUGINS },
      normalizeMathDelimiters(md),
    ),
  )

test('\\( … \\) becomes inline math and is typeset', () => {
  const src = 'The notch gives \\(K_t = 1 + 2b/a = 3.0\\) at the root.'
  assert.equal(normalizeMathDelimiters(src), 'The notch gives $K_t = 1 + 2b/a = 3.0$ at the root.')
  const html = render(src)
  assert.match(html, /class="katex"/)
  assert.doesNotMatch(html, /\\\(/)
})

test('\\[ … \\] on its own line becomes display math', () => {
  const src = 'Bending stress:\n\\[\n\\sigma = \\frac{M c}{I} = 84\\ \\mathrm{MPa}\n\\]\nwithin the allowable.'
  const html = render(src)
  assert.match(html, /katex-display/)
  assert.doesNotMatch(html, /\\\[/)
})

test('\\[ … \\] written inline on one line is still display math', () => {
  const html = render('Fit: \\[ \\varnothing 20\\,\\mathrm{H7/g6} \\]')
  assert.match(html, /katex/)
  assert.doesNotMatch(html, /\\\[/)
})

test('code is never rewritten: fences (card JSON) and inline code spans', () => {
  const fence = '```roshera:refusal\n{"reason": "use \\\\(x\\\\) here"}\n```'
  assert.equal(normalizeMathDelimiters(fence), fence)
  const inline = 'type `\\(x\\)` literally'
  assert.equal(normalizeMathDelimiters(inline), inline)
})

test('an escaped backslash is not a delimiter, an unclosed opener is left alone', () => {
  assert.equal(normalizeMathDelimiters('a \\\\(not math\\\\) b'), 'a \\\\(not math\\\\) b')
  assert.equal(normalizeMathDelimiters('open \\(x + y'), 'open \\(x + y')
})

test('dollar math is untouched (idempotent)', () => {
  const src = 'Mass $m = \\rho V$ and\n$$\nV = 12\\,400\\ \\mathrm{mm^3}\n$$'
  assert.equal(normalizeMathDelimiters(src), src)
  assert.equal(normalizeMathDelimiters(normalizeMathDelimiters('x \\(a\\) y')), 'x $a$ y')
})

test('the streaming splitter withholds then settles \\( … \\) the way it does $ … $', () => {
  const full = 'Stress \\(\\sigma = F/A\\) is fine.'
  const mid = splitStreamingText(normalizeMathDelimiters('Stress \\(\\sigma = F'))
  assert.equal(mid.settled.length, 0)
  assert.equal(mid.pending, 'math', 'an unfinished \\( expression is withheld, not shown raw')
  assert.equal(mid.tail, 'Stress ')
  const midDisplay = splitStreamingText(normalizeMathDelimiters('Check:\n\\[\n\\sigma = M'))
  assert.equal(midDisplay.pending, 'math')
  assert.equal(midDisplay.tail, 'Check:\n')
  const done = splitStreamingText(normalizeMathDelimiters(full))
  assert.deepEqual(done.settled, ['Stress $\\sigma = F/A$'])
  assert.equal(done.tail, ' is fine.')
})

test('splitInlineMath: card text with inline math in every delimiter form', () => {
  assert.deepEqual(splitInlineMath('ISO 286 fit $\\varnothing 20\\,H7/g6$ ok'), [
    { kind: 'text', value: 'ISO 286 fit ' },
    { kind: 'math', value: '\\varnothing 20\\,H7/g6', display: false },
    { kind: 'text', value: ' ok' },
  ])
  assert.deepEqual(splitInlineMath('\\(K_t = 3\\)'), [{ kind: 'math', value: 'K_t = 3', display: false }])
  assert.deepEqual(splitInlineMath('$$m = \\rho V$$'), [{ kind: 'math', value: 'm = \\rho V', display: true }])
})

test('both render paths normalize before they split or render', () => {
  const panels = join(dirname(fileURLToPath(import.meta.url)), '..', 'src', 'components', 'panels')
  const committed = readFileSync(join(panels, 'MessageMarkdown.tsx'), 'utf8')
  assert.match(committed, /normalizeMathDelimiters\(content\)/)
  const streaming = readFileSync(join(panels, 'StreamingLineText.tsx'), 'utf8')
  assert.match(streaming, /splitStreamingText\(normalizeMathDelimiters\(text\)\)/)
})

test('splitInlineMath: prices and lone dollars stay text', () => {
  assert.deepEqual(splitInlineMath('costs $5 and $10 each'), [{ kind: 'text', value: 'costs $5 and $10 each' }])
  assert.deepEqual(splitInlineMath('no math here'), [{ kind: 'text', value: 'no math here' }])
  assert.deepEqual(splitInlineMath('escaped \\$x\\$ stays'), [{ kind: 'text', value: 'escaped \\$x\\$ stays' }])
})
