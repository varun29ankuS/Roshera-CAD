import { test } from 'node:test'
import assert from 'node:assert/strict'

const { presetFor, findFeature, showCaption } = await import('../src/lib/agent-show.ts')

const ch = (anchor, kind, name, instances = []) => ({ kind, name, events: [anchor], anchor, sound: true, refusals: [], author: 'user', author_detail: { kind: 'user' }, intent: null, instances })

test("the agent's view names map to the app's camera presets", () => {
  assert.equal(presetFor('iso'), 'isometric')
  assert.equal(presetFor('front'), 'front')
  assert.equal(presetFor('fit'), null) // Fit frames the model, it is not a preset
})

test('a feature is found by its name, ignoring case and spacing, or by its anchor', () => {
  const chapters = [ch(0, 'cylinder', 'Flange'), ch(7, 'hole', 'Bore'), ch(19, 'pattern', null, [ch(10, 'hole', 'hole1')])]
  assert.equal(findFeature(chapters, { name: '  bore ' })?.anchor, 7)
  assert.equal(findFeature(chapters, { anchor: 19 })?.anchor, 19)
  assert.equal(findFeature(chapters, { name: 'hole1' })?.anchor, 10) // inside a pattern
  assert.equal(findFeature(chapters, { name: 'Gearbox' }), null) // never a guess
})

test('the bar says what is being shown, in the agent words when it gave them', () => {
  assert.equal(showCaption({ action: 'feature', name: 'Bore', caption: 'The bore is 0.1 mm over' }), 'The bore is 0.1 mm over')
  assert.equal(showCaption({ action: 'feature', name: 'Bore' }), 'the feature Bore')
  assert.equal(showCaption({ action: 'workspace', mode: 'drawing' }), 'the Drawing workspace')
  assert.equal(showCaption({ action: 'view', view: 'top' }), 'the top view')
  assert.equal(showCaption({ action: 'faces', solid_id: 12, face_ids: [3, 4] }), '2 faces')
  assert.equal(showCaption({ action: 'step', sequence: 7 }), 'the part after step 7')
})
