// Run: node --experimental-strip-types --test test/sheet-screen-ink.test.mjs
//
// Varun 2026-11-19: "unable to understand what's hidden and what's not". The
// sheet inks ISO 128 widths -- visible 0.5 mm, hidden 0.25 mm dashed -- and
// at Fit an A1 sheet is drawn at ~32%, so both become sub-pixel hairlines in
// the same black. On SCREEN the sheet now draws visible edges at a fixed
// 1.4 px in ink and hidden edges at 0.8 px, grey and clearly dashed, at every
// zoom (non-scaling strokes). Paper (PDF/DXF) keeps the exact ISO widths.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { withScreenInk, SCREEN_INK_CSS } from '../src/lib/sheet-screen-ink.ts'

const here = dirname(fileURLToPath(import.meta.url))
const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 841 594"><style>.view polyline{stroke-width:0.5}</style><g class="view"><polyline points="0,0 1,1"/><polyline class="hidden" points="0,0 1,2"/></g></svg>'

test('the screen styles go inside the sheet, after its own styles', () => {
  const out = withScreenInk(svg)
  const at = out.indexOf(SCREEN_INK_CSS)
  assert.ok(at > 0, 'inserted')
  assert.ok(at > out.indexOf('stroke-width:0.5'), 'after the sheet styles, so they win')
  assert.ok(at < out.indexOf('</svg>'))
})

test('visible and hidden are told apart at any zoom', () => {
  assert.match(SCREEN_INK_CSS, /\.view polyline[^}]*vector-effect:\s*non-scaling-stroke/)
  assert.match(SCREEN_INK_CSS, /\.view polyline\.hidden[^}]*stroke:\s*#[0-9a-f]{6}/i)
  const vis = Number(SCREEN_INK_CSS.match(/\.view polyline,[^{]*\{[^}]*stroke-width:\s*([\d.]+)px/)[1])
  const hid = Number(SCREEN_INK_CSS.match(/\.view polyline\.hidden[^}]*stroke-width:\s*([\d.]+)px/)[1])
  assert.ok(vis >= 1.25 && hid <= vis / 1.5, `visible ${vis}px clearly heavier than hidden ${hid}px`)
})

test('a sheet without markup is returned unchanged', () => {
  assert.equal(withScreenInk(''), '')
})

test('the workspace draws the sheet through it', () => {
  const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')
  assert.ok(ws.includes('withScreenInk(svg)'), 'the on-screen sheet uses the screen ink')
})
