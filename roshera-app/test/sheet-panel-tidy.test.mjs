// Run: node --experimental-strip-types --test test/sheet-panel-tidy.test.mjs
//
// Varun (2026-11-19, rehearsing the Volvo video): "the left column that just
// bunches a lot of dimensions looks very unprofessional". The sheet panel
// showed every dimension of every view as a wall of value chips. Now each
// view is one row with its dimension count, folded by default; opening a
// view lists its dimensions grouped by kind, one per line.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { dimensionGroups } from '../src/lib/sheet-contents.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

const dim = (id, kind, label) => ({ id, kind, label })

test('a view\'s dimensions are grouped by kind, in drawing-reading order', () => {
  const groups = dimensionGroups([
    dim('a', 'radius', 'R25'),
    dim('b', 'extent', '198.8'),
    dim('c', 'diameter', '3× Ø20'),
    dim('d', 'length', '40'),
    dim('e', 'radius', 'R30'),
    dim('f', 'hole_callout', 'Ø13'),
  ])
  assert.deepEqual(
    groups.map((g) => [g.title, g.dimensions.map((d) => d.id)]),
    [
      ['Overall size', ['b']],
      ['Holes', ['f']],
      ['Diameters', ['c']],
      ['Radii', ['a', 'e']],
      ['Lengths', ['d']],
    ],
  )
})

test('an unknown kind still gets a group, after the known ones', () => {
  const groups = dimensionGroups([dim('x', 'thread_pitch', 'P1.5'), dim('y', 'length', '10')])
  assert.deepEqual(groups.map((g) => g.title), ['Lengths', 'Thread pitch'])
})

test('the panel folds each view and lists, not chips', () => {
  const panel = src('components/panels/SheetPanel.tsx')
  assert.ok(panel.includes('dimensionGroups('), 'dimensions are shown grouped')
  assert.ok(!panel.includes('flex flex-wrap gap-1'), 'no wall of wrapped chips')
  assert.ok(/useState<Set<string>>\(\s*\(\)\s*=>\s*new Set\(\)\s*\)/.test(panel), 'views start folded')
})
