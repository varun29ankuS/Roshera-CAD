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
