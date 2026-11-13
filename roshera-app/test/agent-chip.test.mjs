import { test } from 'node:test'
import assert from 'node:assert/strict'

const { agentChip } = await import('../src/lib/agent-chip.ts')

test('a running Claude session reads "Claude · Sonnet"', () => {
  assert.deepEqual(agentChip({ provider: 'anthropic', model: 'sonnet', live: true }), {
    label: 'Claude · Sonnet',
    state: 'live',
  })
})

test('a vendor/model id is shortened the way the header already does', () => {
  assert.deepEqual(
    agentChip({ provider: 'openrouter', model: 'nvidia/nemotron-3-ultra:free', live: false }),
    { label: 'OpenRouter · nemotron-3-ultra', state: 'idle' },
  )
})

test('a provider with no model yet names just the provider', () => {
  assert.deepEqual(agentChip({ provider: 'mistral', model: null, live: false }), {
    label: 'Mistral',
    state: 'idle',
  })
})

test('no provider at all asks to be set up, never invents one', () => {
  assert.deepEqual(agentChip({ provider: null, model: 'sonnet', live: false }), {
    label: 'Set up AI',
    state: 'setup',
  })
})

const { modelOptions } = await import('../src/lib/agent-chip.ts')

test("Claude's drop-down lists just its models", () => {
  assert.deepEqual(modelOptions('anthropic', 'sonnet'), [
    { value: 'opus', label: 'Opus' },
    { value: 'sonnet', label: 'Sonnet' },
    { value: 'haiku', label: 'Haiku' },
  ])
})

test('a saved model outside the list is shown as itself, never as another', () => {
  const opts = modelOptions('anthropic', 'claude-opus-4-1')
  assert.deepEqual(opts?.[0], { value: 'claude-opus-4-1', label: 'claude-opus-4-1' })
  assert.equal(opts?.length, 4)
})

test('a vendor whose models are not known here has no drop-down', () => {
  assert.equal(modelOptions('openrouter', 'some/model'), null)
  assert.equal(modelOptions(null, null), null)
})
