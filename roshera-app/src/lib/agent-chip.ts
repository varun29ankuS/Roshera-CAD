/**
 * The agent chip in the Blackboard's header: which AI is driving, in the
 * words an engineer uses ("Claude · Sonnet"), and the way into its setup.
 * With no provider it says so ("Set up AI") rather than naming one.
 */

const VENDOR_NAMES: Record<string, string> = {
  anthropic: 'Claude',
  openai: 'OpenAI',
  xai: 'Grok',
  mistral: 'Mistral',
  deepseek: 'DeepSeek',
  openrouter: 'OpenRouter',
}

/** `nvidia/nemotron-3:free` -> `nemotron-3`; a bare alias such as `sonnet`
 *  is capitalised (`Sonnet`), a full model id is left as written. */
function shortModel(model: string): string {
  const base = (model.split('/').pop() ?? model).split(':')[0] || model
  return /^[a-z]+$/.test(base) ? base[0].toUpperCase() + base.slice(1) : base
}

export interface AgentChip {
  label: string
  /** live: a session is running; idle: configured, no session; setup: nothing configured. */
  state: 'live' | 'idle' | 'setup'
}

export function agentChip(input: {
  provider: string | null | undefined
  model: string | null | undefined
  live: boolean
}): AgentChip {
  if (!input.provider) return { label: 'Set up AI', state: 'setup' }
  const vendor = VENDOR_NAMES[input.provider] ?? input.provider
  const model = input.model?.trim()
  return {
    label: model ? `${vendor} · ${shortModel(model)}` : vendor,
    state: input.live ? 'live' : 'idle',
  }
}
