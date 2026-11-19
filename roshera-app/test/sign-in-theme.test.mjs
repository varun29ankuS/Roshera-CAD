// Run: node --experimental-strip-types --test test/sign-in-theme.test.mjs
//
// Varun (2026-11-19, rehearsing the Volvo video): "you dont see that you are
// signing in to light space, opening into dark space" -- "match the sign in
// with the theme please". The sign-in was always paper and ink while the app
// opens in its own theme (dark by default). Now the sign-in takes the app's
// theme: its card and words use the app's theme colours, and the drawn
// aircraft has a light palette (ink on paper) and a dark one (light linework
// on graphite).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { signInScenePalette } from '../src/lib/sign-in-palette.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

/** Relative luminance of #rrggbb, 0 (black) .. 1 (white). */
function lum(hex) {
  const n = parseInt(hex.slice(1), 16)
  const c = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => {
    const s = v / 255
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

test('light: dark ink on a pale skin', () => {
  const p = signInScenePalette('light')
  assert.ok(lum(p.skin) > 0.7, 'pale skin')
  for (const k of ['crease', 'rim', 'silhouette']) assert.ok(lum(p[k]) < 0.05, `${k} is ink-dark`)
})

test('dark: light linework on a graphite skin', () => {
  const p = signInScenePalette('dark')
  assert.ok(lum(p.skin) < 0.05, 'graphite skin')
  for (const k of ['crease', 'rim', 'silhouette']) assert.ok(lum(p[k]) > 0.5, `${k} is light`)
  assert.ok(lum(p.wakePale) > lum(signInScenePalette('light').wakePale) - 0.05, 'the wake stays bright on dark')
})

test('the sign-in takes the app theme', () => {
  const dialog = src('components/LoginDialog.tsx')
  assert.ok(!dialog.includes('bg-[#f4f1eb]'), 'no fixed paper background')
  assert.ok(dialog.includes('bg-background') && dialog.includes('text-foreground'), 'the app theme colours')
  assert.ok(dialog.includes('bg-card'), 'the card is a theme card')
  const hero = src('components/SignInHero.tsx')
  assert.ok(hero.includes('signInScenePalette('), 'the aircraft is drawn in the theme palette')
  assert.ok(hero.includes('useThemeStore'), 'it reads the app theme')
})
