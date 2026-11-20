// Run: node --experimental-strip-types --test test/card-math.test.mjs
//
// Cards printed their authored text raw: a note "$K_t = 1 + 2b/a$" showed
// its dollar signs. Authored fields (notes, hints, next actions, table
// labels) now typeset inline math through `cardTextParts`; verbatim kernel
// strings (a refusal's reason, a certificate's verdict) stay exactly as the
// kernel wrote them.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const { cardTextParts } = await import('../src/lib/card-math.ts')
const cards = join(dirname(fileURLToPath(import.meta.url)), '..', 'src', 'components', 'panels', 'cards')
const read = (f) => readFileSync(join(cards, f), 'utf8')

test('inline math in card text is typeset with KaTeX', () => {
  const parts = cardTextParts('Peterson gives $K_t = 1 + 2b/a = 3.0$ at the notch')
  assert.equal(parts.length, 3)
  assert.equal(parts[0].value, 'Peterson gives ')
  assert.equal(parts[1].kind, 'math')
  assert.match(parts[1].html, /class="katex"/)
  assert.equal(parts[2].value, ' at the notch')
})

test('malformed LaTeX degrades to its source, never throws', () => {
  const parts = cardTextParts('bad $\\frac{1$ here')
  assert.ok(parts.some((p) => p.kind === 'math' && /katex-error|\\frac/.test(p.html)))
})

test('KaTeX trust stays off: \\href is refused, not rendered as a link', () => {
  const [p] = cardTextParts('$\\href{javascript:alert(1)}{x}$')
  assert.doesNotMatch(p.html, /<a /)
})

test('the cards route authored fields through CardText, verbatim reasons stay plain', () => {
  const refusal = read('RefusalCard.tsx')
  assert.match(refusal, /<CardText text=\{card\.hint\} \/>/)
  assert.match(refusal, /<CardText text=\{opt\} \/>/)
  assert.doesNotMatch(refusal, /<CardText text=\{card\.reason\}/)
  assert.match(read('DfmCard.tsx'), /<CardText text=\{card\.note\} \/>/)
  assert.match(read('FcfCard.tsx'), /<CardText text=\{card\.note\} \/>/)
  for (const f of ['PartCard.tsx', 'SheetCard.tsx', 'MeasuresCard.tsx']) {
    assert.match(read(f), /<CardText text=\{card\.note\} \/>/, f)
  }
  assert.match(read('MeasuresCard.tsx'), /<CardText text=\{r\.quantity\} \/>/)
})

test('the renderer dispatches every new kind to its card', () => {
  const renderer = read('CardRenderer.tsx')
  for (const [kind, comp] of [['part', 'PartCard'], ['sheet', 'SheetCard'], ['measures', 'MeasuresCard']]) {
    assert.match(renderer, new RegExp(`case '${kind}':\\s*return <${comp} card=\\{card\\.card\\} />`), kind)
  }
})
