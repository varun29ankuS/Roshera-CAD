/**
 * Switch the agent's model from the Blackboard's drop-down: save the new
 * model for the provider and sign-in method already in use, then restart
 * the agent so it runs on it -- the same steps the AI pop-up's Save takes
 * (`putProvider`, drop the old connection, start a fresh one).
 */
import { getProviderStatus, putProvider, type CredentialMode } from '@/lib/provider-api'
import { establishAcpSession, resetAcpClient } from '@/lib/acp-blackboard'
import { useAgentUiStore } from '@/stores/agent-ui-store'

/** Resolves to null on success, or a sentence saying what failed. */
export async function switchAgentModel(model: string): Promise<string | null> {
  const configured = useAgentUiStore.getState().configured
  if (!configured) return 'No AI is set up yet — open the AI pop-up to choose one.'
  const res = await putProvider({
    provider: configured.provider,
    mode: configured.mode as CredentialMode,
    model,
    // The same provider and sign-in method are already in use: the user
    // consented when they were saved, as the pop-up also assumes.
    consent_spawn_local_process: true,
  })
  if (!res.ok) {
    return res.kind === 'unavailable'
      ? 'Could not save the model: the server is not answering.'
      : `Could not switch to ${model}: ${[res.message, res.hint].filter(Boolean).join(' — ')}`
  }
  // The server ended the old connection when the choice was saved; start
  // a fresh one on the new model.
  resetAcpClient()
  const status = await getProviderStatus()
  if (status.ok) {
    const a = status.data.active
    useAgentUiStore
      .getState()
      .setConfigured(a ? { provider: a.provider, mode: a.mode, model: a.model ?? null } : null)
  }
  try {
    await establishAcpSession()
  } catch (err) {
    return `Saved ${model}, but the agent did not restart: ${err instanceof Error ? err.message : String(err)}`
  }
  return null
}
