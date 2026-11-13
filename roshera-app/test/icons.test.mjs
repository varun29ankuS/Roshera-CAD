// Run: node --experimental-strip-types --test test/icons.test.mjs
//
// Roshera's icons are rendered natively at each size (never a stretched
// bitmap). These tests pin which rendered size an icon drawn at a given CSS
// size uses, and that every icon the app references exists on disk at every
// size -- a missing or misspelt icon is a broken image in the toolbar.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { drawnSize, renderedSize } from '../src/components/ui/icon-size.ts'
import { ICON_SIZES, TOOL_ICON_NAMES, UI_ICON_NAMES } from '../src/components/ui/icon-manifest.ts'

const APP = join(dirname(fileURLToPath(import.meta.url)), '..')

test('an icon is drawn at its size prop, else its Tailwind size class, else 24', () => {
  assert.equal(drawnSize(22, 'h-4 w-4'), 22)
  assert.equal(drawnSize('18', undefined), 18)
  assert.equal(drawnSize(undefined, 'shrink-0 h-3.5 w-3.5'), 14)
  assert.equal(drawnSize(undefined, 'size-5'), 20)
  assert.equal(drawnSize(undefined, 'w-[18px] h-[18px]'), 18)
  assert.equal(drawnSize(undefined, 'text-red-500'), 24)
  assert.equal(drawnSize(undefined, undefined), 24)
})

test('the rendered size is the smallest native render at least as big -- never stretched up', () => {
  assert.equal(renderedSize(14), 16)
  assert.equal(renderedSize(16), 16)
  assert.equal(renderedSize(22), 24)
  assert.equal(renderedSize(28), 32) // a 14 px icon on a 2x screen
  assert.equal(renderedSize(44), 48)
  assert.equal(renderedSize(100), 64) // beyond the largest: the largest render
})

test('every icon the app references is rendered at every size', () => {
  const src = readFileSync(join(APP, 'src/components/ui/icons.tsx'), 'utf8')
  const used = [
    ...[...src.matchAll(/toolIcon\('([a-z0-9-]+)'\)/g)].map((m) => ['tools', m[1]]),
    ...[...src.matchAll(/uiIcon\('([a-z0-9-]+)'\)/g)].map((m) => ['ui', m[1]]),
  ]
  assert.ok(used.length > 90, `expected the full set, found ${used.length}`)
  const missing = []
  for (const [family, name] of used) {
    const known = family === 'tools' ? TOOL_ICON_NAMES : UI_ICON_NAMES
    if (!known.includes(name)) missing.push(`${family}/${name}: not in the manifest`)
    for (const px of ICON_SIZES) {
      const file = join(APP, 'public/icons', family, `${name}-${px}.png`)
      if (!existsSync(file)) missing.push(`${family}/${name}-${px}.png`)
    }
  }
  assert.deepEqual(missing, [])
})
