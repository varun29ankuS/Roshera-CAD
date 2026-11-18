// Run: node --experimental-strip-types --test test/first-run.test.mjs
//
// A first-time user must know what to do (Varun 2026-11-18: "there is also
// no first time sign in explanations ... no way for anyone to know what to
// do"). lib/first-run.ts holds the getting-started steps, when the guide
// shows (once per browser until dismissed; reopenable), and the sign-in
// screen's wording for failures. Against the source: the guide is mounted
// in the app, reopenable from the command palette, and the sign-in screen
// uses the plain-language errors.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  GUIDE_STEPS,
  GUIDE_SEEN_KEY,
  shouldShowGuide,
  markGuideSeen,
  friendlyLoginError,
  parseShowcase,
  SHOWCASE_LABEL,
} from '../src/lib/first-run.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

function memoryStorage() {
  const m = new Map()
  return {
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => m.set(k, String(v)),
  }
}

test('the guide walks through the four things a first-time engineer needs', () => {
  assert.equal(GUIDE_STEPS.length, 4)
  const all = GUIDE_STEPS.map((s) => `${s.title} ${s.body} ${s.where}`).join(' ')
  for (const must of ['agent', 'Agent sees', '✓', 'Drawing', 'STEP']) {
    assert.ok(all.includes(must), `the steps mention "${must}"`)
  }
  for (const s of GUIDE_STEPS) {
    assert.ok(s.title.length <= 32, `title is a label, not a sentence: "${s.title}"`)
    assert.ok(s.body.length <= 170, `body is a card, not a paragraph (${s.body.length} chars): "${s.body}"`)
    assert.ok(s.where.length > 0, `each step says where to look: "${s.title}"`)
  }
})

test('the guide shows once, until it is dismissed', () => {
  const storage = memoryStorage()
  assert.equal(shouldShowGuide(storage), true, 'a first visit shows it')
  markGuideSeen(storage)
  assert.equal(storage.getItem(GUIDE_SEEN_KEY) !== null, true)
  assert.equal(shouldShowGuide(storage), false, 'a returning visit does not')
})

test('storage that throws neither crashes nor hides the guide', () => {
  const broken = {
    getItem: () => {
      throw new Error('blocked')
    },
    setItem: () => {
      throw new Error('blocked')
    },
  }
  assert.equal(shouldShowGuide(broken), true)
  assert.doesNotThrow(() => markGuideSeen(broken))
})

test('sign-in failures are said in plain words', () => {
  assert.match(friendlyLoginError('Invalid credentials'), /username and password don.t match/i)
  assert.match(friendlyLoginError('Login failed (401)'), /username and password don.t match/i)
  assert.match(friendlyLoginError('Failed to fetch'), /can.t reach the Roshera server/i)
  assert.match(friendlyLoginError('NetworkError when attempting to fetch resource.'), /can.t reach the Roshera server/i)
  assert.match(friendlyLoginError('Login failed (429)'), /too many attempts/i)
  assert.match(friendlyLoginError('Login failed (500)'), /server could not sign you in/i)
})

test('the guide is mounted in the app and reopenable from the command palette', () => {
  const app = src('App.tsx')
  assert.ok(/<FirstRunGuide\s*\/>/.test(app), 'App mounts <FirstRunGuide />')
  const palette = src('components/CommandPalette.tsx')
  assert.ok(palette.includes("'help.getting-started'"), 'a command reopens the guide')
  assert.ok(palette.includes('openGuide('), 'and it calls openGuide()')
})

test('the sign-in screen explains Roshera and uses the plain-language errors', () => {
  const login = src('components/LoginDialog.tsx')
  assert.ok(login.includes('friendlyLoginError('), 'errors go through friendlyLoginError')
  assert.ok(!login.includes('This instance requires authentication'), 'no system-speak')
  assert.ok(/<label/.test(login), 'fields are labelled, not placeholder-only')
  assert.ok(login.includes('/logo.png'), 'it carries the Roshera logo')
  assert.ok(login.includes('parseShowcase('), 'it shows the showcase from its manifest')
})

test('the showcase manifest keeps only real, well-formed items', () => {
  const items = parseShowcase({
    items: [
      { kind: 'drawing', title: 'Bracket', caption: 'Certified sheet', src: '/showcase/a.png' },
      { kind: 'part', title: 'Flange', caption: 'Sound', src: '/showcase/b.png' },
      { kind: 'assembly', title: 'Vice', caption: '12 parts', src: '/showcase/c.png' },
      { kind: 'video', title: 'x', caption: 'y', src: '/showcase/d.mp4' },
      { kind: 'part', title: 'Remote', caption: 'z', src: 'https://elsewhere.example/e.png' },
      { kind: 'part', title: '', caption: 'no title', src: '/showcase/f.png' },
      'not an object',
    ],
  })
  assert.deepEqual(
    items.map((i) => i.title),
    ['Bracket', 'Flange', 'Vice'],
    'unknown kinds, other hosts and untitled items are dropped',
  )
  assert.deepEqual(parseShowcase(null), [])
  assert.deepEqual(parseShowcase({ items: 'nope' }), [])
  assert.equal(SHOWCASE_LABEL.assembly, 'Assembly')
})
