// Run: node --experimental-strip-types --test test/feature-map.test.mjs
//
// The timeline Map is one feature graph (Varun 2026-11-18: "one look at map
// should tell everything ... how it was made .. what was agent / what was
// human ... can it be used to rebuild the part"). These pin the pure layer:
// left-to-right build order, the summary line, who made each feature, and
// that the old three-tab Map is gone.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { layoutFeatureGraph, summaryLine, authorMark, rebuildLine } from '../src/lib/feature-graph.ts'

const here = dirname(fileURLToPath(import.meta.url))
const panels = join(here, '..', 'src', 'components', 'panels')

const f = (anchor, extra = {}) => ({ anchor, author: 'agent', author_detail: { kind: 'agent', model: 'claude-opus' }, ...extra })

test('the layout reads left to right in build order', () => {
  const features = [f(0), f(3), f(7)]
  const edges = [
    { from: 0, to: 3, via: 'the Flange body' },
    { from: 3, to: 7, via: 'the Hub' },
  ]
  const at = layoutFeatureGraph(features, edges)
  assert.ok(at.get(0).x < at.get(3).x && at.get(3).x < at.get(7).x, JSON.stringify([...at]))
})

test('features with no wires still read in build order', () => {
  const at = layoutFeatureGraph([f(5), f(1), f(9)], [])
  assert.ok(at.get(1).x < at.get(5).x && at.get(5).x < at.get(9).x, JSON.stringify([...at]))
})

test('the summary line counts the agent and people, and what rebuilds', () => {
  assert.equal(
    summaryLine({ features: 6, by_agent: 4, by_user: 2, steps_rebuildable: 35, steps_total: 51, set_aside: 16 }),
    '6 features · 4 ✦ agent · 2 by people · rebuilds ✓ 35/51 steps',
  )
})

test('who made it: the agent is ✦ with its model, a person their initials', () => {
  const a = authorMark(f(0))
  assert.equal(a.glyph, '✦')
  assert.ok(a.title.includes('claude-opus'))
  const u = authorMark({ anchor: 1, author: 'user', author_detail: { kind: 'user', name: 'Varun Sharma' } })
  assert.equal(u.glyph, 'VS')
  assert.ok(u.title.includes('Varun Sharma'))
  assert.equal(authorMark({ anchor: 2, author: 'system', author_detail: { kind: 'system' } }).glyph, '·')
})

test('the rebuild result reads identical only when the kernel measured it so', () => {
  assert.equal(
    rebuildLine({ identical: true, parts: [{ name: 'Flange', differences: [], live: { faces: 42 } }], steps_replayed: 12 }),
    'Rebuilt from the recipe: identical ✓ — volume, area, box and 42 faces match (12 steps replayed)',
  )
  assert.equal(
    rebuildLine({ identical: false, parts: [{ name: 'Flange', differences: ['volume 1000 vs 990'], live: { faces: 42 } }], steps_replayed: 12 }),
    'Rebuilt from the recipe: differs ✗ — Flange: volume 1000 vs 990',
  )
})

test('the Map has one view: the old three-tab Map is gone', () => {
  assert.ok(!existsSync(join(panels, 'TimelineGraph.tsx')), 'TimelineGraph.tsx deleted')
  const timeline = readFileSync(join(panels, 'Timeline.tsx'), 'utf8')
  assert.ok(timeline.includes("import('./FeatureMap')"), 'Timeline opens the FeatureMap')
  const map = readFileSync(join(panels, 'FeatureMap.tsx'), 'utf8')
  assert.ok(!/'lanes'|'decisions'/.test(map), 'no tabs')
  assert.ok(!/solid:\$\{/.test(map), 'no kernel ids in default text')
  assert.ok(map.includes('Rebuild & certify') && map.includes('Recipe'), 'both rebuild paths are there')
})
