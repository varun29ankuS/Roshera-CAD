// Run: node --experimental-strip-types --test test/markdown-tables.test.mjs
//
// An agent's table of measured values ("| quantity | value |") rendered as
// literal pipe text on the Blackboard: the markdown pipeline had no GFM, so
// tables were never parsed. These render the panel's own plugin lists
// (src/lib/markdown-pipeline.ts) through react-markdown to static HTML.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import ReactMarkdown from 'react-markdown'

const { REMARK_PLUGINS, REHYPE_PLUGINS } = await import('../src/lib/markdown-pipeline.ts')
const here = dirname(fileURLToPath(import.meta.url))

const render = (md) =>
  renderToStaticMarkup(
    createElement(ReactMarkdown, { remarkPlugins: REMARK_PLUGINS, rehypePlugins: REHYPE_PLUGINS }, md),
  )

const TABLE = [
  '| Quantity | Value | Limit |',
  '|---|---:|---:|',
  '| Wall thickness | 2.40 mm | 2.0 mm |',
  '| Stress concentration | $K_t = 1 + 2b/a = 3.0$ | — |',
].join('\n')

test('a GFM table renders as a table, not pipe text', () => {
  const html = render(TABLE)
  assert.match(html, /<table>/)
  assert.match(html, /<th[^>]*>Quantity<\/th>/)
  assert.match(html, /<td[^>]*>Wall thickness<\/td>/)
  assert.doesNotMatch(html, /\| Quantity \|/)
})

test('math inside a table cell is still typeset (GFM and remark-math together)', () => {
  const html = render(TABLE)
  const cell = html.split('<td').slice(1).find((c) => c.includes('katex'))
  assert.ok(cell, 'a cell carries KaTeX output')
  assert.doesNotMatch(html, /\$K_t/, 'the raw $ source is gone')
})

test('a single ~ stays "approximately", never strikethrough', () => {
  // With GFM's default (singleTilde: true) both lines strike through.
  const range = render('Bore 4~6 mm, boss 8~10 mm.')
  assert.doesNotMatch(range, /<del>/)
  assert.match(range, /4~6 mm, boss 8~10 mm/)
  assert.doesNotMatch(render('wall ~5 mm and rib~ 3'), /<del>/)
  assert.match(render('~~withdrawn~~'), /<del>withdrawn<\/del>/, 'double tilde still strikes through')
})

test('display and inline math still render alongside GFM', () => {
  const html = render('Mass $m = \\rho V$:\n\n$$\nm = 7.85 \\times 10^{-6} \\times 12\\,400 = 0.097\\ \\mathrm{kg}\n$$')
  assert.match(html, /katex-display/)
  assert.ok((html.match(/class="katex"/g) ?? []).length >= 2)
})

test('the panel renders with the shared pipeline and styles tables', () => {
  const src = readFileSync(join(here, '..', 'src', 'components', 'panels', 'MessageMarkdown.tsx'), 'utf8')
  assert.match(src, /remarkPlugins=\{REMARK_PLUGINS\}/)
  assert.match(src, /rehypePlugins=\{REHYPE_PLUGINS\}/)
  for (const el of ['table', 'thead', 'th', 'td', 'tr']) {
    assert.match(src, new RegExp(`\\b${el}: \\(`), `a ${el} handler styles the table`)
  }
})
