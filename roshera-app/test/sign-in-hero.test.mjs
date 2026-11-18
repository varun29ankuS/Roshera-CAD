// Run: node --experimental-strip-types --test test/sign-in-hero.test.mjs
//
// The sign-in screen (Varun 2026-11-18): an ink-and-paper stealth fighter
// concept with a particle wake, in the manner of nTop's hero; the copy says
// what Roshera does. The aircraft's numbers live in lib/fighter-model and are
// checked like geometry: the NACA section, the planform, and that every wake
// particle starts on a trailing edge, a nozzle exit or a wingtip.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  nacaThickness,
  nacaSection,
  WING,
  NOZZLE,
  wingLeadingEdgeX,
  wingTrailingEdgeX,
  wakeSeeds,
  WAKE_KIND,
} from '../src/lib/fighter-model.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

test('the NACA 4-digit thickness is the published one', () => {
  // NACA 0012 at x/c = 0.3: half-thickness 0.06002 c (closed-TE coefficient)
  assert.ok(Math.abs(nacaThickness(0.12, 0.3) - 0.06002) < 2e-4)
  assert.equal(nacaThickness(0.12, 0), 0)
  const loop = nacaSection(0.045, 20)
  assert.ok(loop.length > 30, 'a closed loop of points')
  for (const [x] of loop) assert.ok(x >= 0 && x <= 1)
})

test('the planform: 42 degree leading edge, cranked trailing edge, tip inside the span', () => {
  const sweep = Math.atan((wingLeadingEdgeX(WING.tipZ) - wingLeadingEdgeX(WING.rootZ)) / (WING.tipZ - WING.rootZ))
  assert.ok(Math.abs((sweep * 180) / Math.PI - 42) < 1e-9, `sweep ${(sweep * 180) / Math.PI}`)
  assert.ok(wingTrailingEdgeX(WING.crankZ) < wingTrailingEdgeX(WING.rootZ), 'inner trailing edge sweeps slightly forward')
  assert.ok(wingTrailingEdgeX(WING.tipZ) < wingTrailingEdgeX(WING.crankZ), 'outer panel swept forward')
  assert.ok(wingTrailingEdgeX(WING.tipZ) > wingLeadingEdgeX(WING.tipZ), 'the tip has chord')
})

test('every wake particle starts on a trailing edge, a nozzle exit or a wingtip', () => {
  let n = 0
  const rand = () => ((n = (n * 16807 + 12345) % 2147483647) / 2147483647)
  const { origin, seed, count } = wakeSeeds(4000, rand)
  assert.equal(count, 4000)
  const kinds = new Set()
  // positions are float32 (GPU buffers): compare to a tenth of a millimetre
  const EPS = 1e-4
  for (let i = 0; i < count; i++) {
    const x = origin[i * 3], y = origin[i * 3 + 1], z = origin[i * 3 + 2], kind = seed[i * 4 + 2]
    kinds.add(kind)
    if (kind === WAKE_KIND.wing) {
      assert.ok(Math.abs(z) >= WING.rootZ - EPS && Math.abs(z) <= WING.tipZ + EPS, 'on the span')
      assert.ok(Math.abs(x - wingTrailingEdgeX(Math.abs(z))) < EPS, 'on the trailing edge')
    } else if (kind === WAKE_KIND.nozzle) {
      const r = Math.hypot(y - NOZZLE.y, Math.abs(z) - NOZZLE.z)
      assert.ok(r <= NOZZLE.exitRadius + EPS, `inside the exit: ${r}`)
    } else if (kind === WAKE_KIND.tip) {
      assert.ok(Math.abs(Math.abs(z) - WING.tipZ) < EPS, 'at the tip')
    }
  }
  assert.ok(kinds.has(WAKE_KIND.wing) && kinds.has(WAKE_KIND.nozzle) && kinds.has(WAKE_KIND.tip))
})

test('the sign-in screen: the fighter, words that say what Roshera does, nothing fake', () => {
  const login = src('components/LoginDialog.tsx')
  assert.ok(login.includes('<SignInHero'), 'the fighter is the hero')
  assert.ok(login.includes('Describe the part.') && login.includes('Get a verified model.'), 'the headline')
  assert.ok(login.includes('friendlyLoginError('), 'failures in plain words')
  assert.ok(login.includes("'/health'") || login.includes('/health`'), 'the status reads the real server')
  assert.ok(!/✓\s*sound/i.test(login), 'no kernel verdict claimed before anything was checked')
  assert.ok(!/Keep me signed in|Request access/.test(login), 'no control that does nothing')
  assert.ok(!existsSync(join(here, '..', 'src', 'components', 'CouplingHero.tsx')), 'the coupling hero is gone')
})
