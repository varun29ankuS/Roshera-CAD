// Run: node --experimental-strip-types --test test/dimension-annotate.test.mjs
//
// The drawing tab's dimension property panel: the pure part. These pin what
// the panel's fields mean as the one PATCH it sends
// (`/api/drawings/{id}/dimensions/{dim}`), what is refused before the server
// is asked, what a drag of a dimension becomes, and what the certificate chip
// says. The wire shapes are the kernel's (`drawing/dimension_edit.rs`):
// `{type:'bilateral',upper,lower}` etc. -- a drift here is a 422 in the app.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  FIT_GRADES,
  FIT_LETTERS,
  MAX_DIM_TEXT,
  TOLERANCE_FORMS,
  certificateChip,
  draftOf,
  draggedShift,
  fitDesignation,
  omissionReason,
  parseNumber,
  textRefusal,
  toleranceOf,
  withChip,
} from '../src/lib/dimension-annotate.ts'

const blank = draftOf(null)

test('every tolerance form the kernel stores is offered, and round-trips through the draft', () => {
  assert.deepEqual(
    TOLERANCE_FORMS.map((f) => f.key),
    ['none', 'symmetric', 'bilateral', 'limits', 'fit', 'basic', 'reference'],
  )
  const stored = [
    { type: 'symmetric', pm: 0.15 },
    { type: 'bilateral', upper: 0.05, lower: -0.1 },
    { type: 'bilateral', upper: 0.1, lower: 0.05 },
    { type: 'limits', upper: 25.15, lower: 24.85 },
    { type: 'fit', designation: 'H7' },
    { type: 'fit', designation: 'g6' },
    { type: 'fit', designation: 'JS9' },
    { type: 'basic' },
    { type: 'reference' },
  ]
  for (const t of stored) {
    const back = toleranceOf(draftOf(t))
    assert.deepEqual(back, { ok: true, tolerance: t }, JSON.stringify(t))
  }
  assert.deepEqual(toleranceOf(blank), { ok: true, tolerance: null }, 'None removes the tolerance')
})

test('a fit is named hole-upper / shaft-lower from the pickers', () => {
  assert.equal(fitDesignation({ fitKind: 'hole', fitLetter: 'h', fitGrade: '7' }), 'H7')
  assert.equal(fitDesignation({ fitKind: 'shaft', fitLetter: 'g', fitGrade: '6' }), 'g6')
  assert.equal(fitDesignation({ fitKind: 'hole', fitLetter: 'js', fitGrade: '9' }), 'JS9')
  // The pickers offer only what the kernel's ISO 286 tables carry.
  assert.ok(FIT_LETTERS.includes('h') && FIT_LETTERS.includes('g') && !FIT_LETTERS.includes('za'))
  assert.ok(FIT_GRADES.includes('6') && FIT_GRADES.includes('7'))
})

test('numbers are read as an engineer types them', () => {
  assert.equal(parseNumber('0.1'), 0.1)
  assert.equal(parseNumber('+0.05'), 0.05)
  assert.equal(parseNumber('-0,10'), -0.1)
  assert.equal(parseNumber(' 25.15 '), 25.15)
  for (const bad of ['', 'abc', '1e3', '0.1mm', '--1', '1.2.3']) assert.equal(parseNumber(bad), null, bad)
})

test('fields that are not a tolerance are refused before the server is asked', () => {
  const refused = (d) => {
    const r = toleranceOf(d)
    assert.equal(r.ok, false, JSON.stringify(d))
    assert.ok(r.reason.length > 10)
  }
  refused({ ...blank, form: 'symmetric', pm: '' })
  refused({ ...blank, form: 'symmetric', pm: '0' })
  refused({ ...blank, form: 'symmetric', pm: '-0.1' })
  refused({ ...blank, form: 'bilateral', upper: '0.05', lower: '' })
  refused({ ...blank, form: 'bilateral', upper: '-0.1', lower: '0.05' })
  refused({ ...blank, form: 'limits', upper: '24.85', lower: '25.15' })
  // Both deviations positive is a real tolerance (25 +0.10/+0.05).
  assert.deepEqual(toleranceOf({ ...blank, form: 'bilateral', upper: '+0.10', lower: '+0.05' }), {
    ok: true,
    tolerance: { type: 'bilateral', upper: 0.1, lower: 0.05 },
  })
})

test('dimension text: ordinary drafting text is accepted, a thread designation included', () => {
  for (const ok of ['4X', 'TYP', 'THRU', 'M12x1.75-6H', 'Ra 3.2', 'SEE NOTE 3', 'DIN 74', ''])
    assert.equal(textRefusal(ok), null, ok)
  assert.ok(textRefusal('x'.repeat(MAX_DIM_TEXT + 1)))
  assert.ok(textRefusal('two\nlines'))
  // A word chip gets a space, a symbol does not; never past the limit.
  assert.equal(withChip('', 'TYP'), 'TYP')
  assert.equal(withChip('4X', 'THRU'), '4X THRU')
  assert.equal(withChip('4X ', 'Ø'), '4X Ø')
  assert.equal(withChip('x'.repeat(MAX_DIM_TEXT), 'TYP').length, MAX_DIM_TEXT)
})

test('a drag becomes the shift from where the layout puts the dimension', () => {
  // 4 screen px per sheet mm; right and down on screen are + on the sheet.
  assert.deepEqual(draggedShift(null, 40, 20, 4), [10, 5])
  // A second drag adds to the recorded shift.
  assert.deepEqual(draggedShift([10, 5], -8, 4, 4), [8, 6])
  // Dragged back to where the layout puts it: the shift is cleared.
  assert.equal(draggedShift([10, 5], -40, -20, 4), null)
  // 0.01 mm resolution, never -0.
  assert.deepEqual(draggedShift(null, 1, -1, 3), [0.33, -0.33])
  assert.ok(Object.is(draggedShift([1, 0], 0, 0.001, 4)[1], 0))
})

test('the chip says in one glance whether the sheet is sound and complete', () => {
  const counts = { stale: 0, dangling: 0, omitted: 0, not_dimensioned: 0, waived: 0 }
  const cert = (over) => ({ sound: true, complete: true, counts, omission_summary: [], omissions: [], ...over })
  assert.deepEqual(certificateChip(cert({})), { tone: 'good', mark: '✓', text: 'Sound and complete' })
  assert.equal(
    certificateChip(cert({ counts: { ...counts, waived: 2 } })).text,
    'Sound and complete (2 waived)',
  )
  const incomplete = certificateChip(cert({ complete: false, counts: { ...counts, not_dimensioned: 3 } }))
  assert.equal(incomplete.tone, 'warn')
  assert.equal(incomplete.text, 'Sound, incomplete: 3 not dimensioned')
  const unsound = certificateChip(cert({ sound: false, counts: { ...counts, stale: 1, dangling: 1 } }))
  assert.equal(unsound.tone, 'bad')
  assert.equal(unsound.text, 'Not sound: 2 to fix')
  // No certificate (the part is not loaded) is never shown as a pass.
  assert.equal(certificateChip(null).tone, 'warn')
})

test('every omission reason the kernel emits has words', () => {
  for (const r of [
    'angle_clutter',
    'merged_with_equal_value',
    'diameter_limit',
    'view_limit',
    'not_placed',
    'suppressed',
    'not_on_sheet',
  ]) {
    const words = omissionReason(r)
    assert.ok(words.length > 5 && !words.includes('_'), `${r} -> ${words}`)
  }
})
