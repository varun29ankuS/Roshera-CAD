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

/** The models Claude offers, as the drop-downs list them. */
export const CLAUDE_MODELS: { value: string; label: string }[] = [
  { value: 'opus', label: 'Opus' },
  { value: 'sonnet', label: 'Sonnet' },
  { value: 'haiku', label: 'Haiku' },
]

/**
 * The model drop-down's entries for a provider: just the models available.
 * A saved model outside the list leads it, shown as itself -- never
 * displayed as a different model. `null` for a vendor whose model list is
 * not known here (its models are chosen in the AI pop-up).
 */
export function modelOptions(
  provider: string | null | undefined,
  current: string | null | undefined,
): { value: string; label: string }[] | null {
  if (provider !== 'anthropic') return null
  const cur = current?.trim() ?? ''
  if (cur && !CLAUDE_MODELS.some((m) => m.value === cur)) {
    return [{ value: cur, label: cur }, ...CLAUDE_MODELS]
  }
  return [...CLAUDE_MODELS]
}
