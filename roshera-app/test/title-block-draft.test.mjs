// Run: node --experimental-strip-types --test test/title-block-draft.test.mjs
//
// Found rehearsing the Volvo video (2026-11-19): in the title block, each
// field saves when you leave it, and the parent remounted the whole card
// whenever the server's title block changed. Typing "Created by", tabbing to
// "Approved by" and typing there lost the second field: the first field's
// save landed, the card remounted from the server, and the half-typed name
// (and the focus) were gone.
//
// The card now stays mounted. When the server's title block changes, a
// field takes the server's value only if the person has not edited it since
// the last server value.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { mergeTitleBlockDraft } from '../src/lib/title-block-draft.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

test('a field being typed survives the save of another field', () => {
  const before = { drawn_by: '', approved_by: '', material: '' }
  // The person typed both; only "Created by" has been saved so far.
  const draft = { drawn_by: 'Roshera', approved_by: 'Demo eng', material: '' }
  const server = { drawn_by: 'Roshera', approved_by: '', material: '' }
  assert.deepEqual(mergeTitleBlockDraft(before, draft, server), {
    drawn_by: 'Roshera',
    approved_by: 'Demo eng',
    material: '',
  })
})

test('a field the person did not touch takes the server value', () => {
  const before = { material: '', revision: 'A' }
  const draft = { material: '', revision: 'A' }
  // The kernel (or another person) changed the revision.
  const server = { material: '', revision: 'B' }
  assert.deepEqual(mergeTitleBlockDraft(before, draft, server), { material: '', revision: 'B' })
})

test('the card is not remounted each time the title block saves', () => {
  const ws = src('components/panels/DrawingsWorkspace.tsx')
  const at = ws.indexOf('<TitleBlockCard')
  assert.ok(at > 0, 'the workspace shows the title block card')
  const tag = ws.slice(at, ws.indexOf('/>', at))
  assert.ok(!/key=\{[^}]*title_block/.test(tag), 'its key does not change with the saved title block')
  const card = src('components/panels/drawing-overlays.tsx')
  assert.ok(card.includes('mergeTitleBlockDraft('), 'the card merges server changes into the draft')
})
