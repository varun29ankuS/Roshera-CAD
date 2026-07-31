import { useState } from 'react'
import { CheckCircle2, Loader2, Terminal, XCircle } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import {
  deleteProvider,
  getProviderStatus,
  putProvider,
  testProvider,
  type AllowlistedProvider,
  type CliDetection,
  type CredentialMode,
  type ModeEntry,
  type ProviderStatusResponse,
} from '@/lib/provider-api'

/** `GET /api/ai/provider`'s `cli` object is keyed by CLI, not by provider
 *  id (`ai_provider_config::detect_claude_cli` / `detect_codex_cli`). */
const CLI_KEY_FOR_PROVIDER: Record<string, 'claude' | 'codex'> = {
  anthropic: 'claude',
  openai: 'codex',
}

/**
 * AI PROVIDER SETTINGS — trigger button + dialog
 * ================================================
 * Self-contained: `<ProviderSettingsButton />` is the only export TopBar's
 * right cluster needs. Renders the server-owned provider allowlist
 * (`ai-integration/src/providers/allowlist.rs`, mirrored verbatim in
 * `lib/provider-api.ts`) and lets the user select and configure a mode.
 *
 * The fetch-and-reset-on-open logic runs from the trigger button's
 * `onClick` (a real event handler), not a `useEffect` keyed on `open` —
 * `react-hooks/set-state-in-effect` (this project's eslint config)
 * correctly flags synchronous `setState` in an Effect body as a
 * cascading-render smell; opening a dialog is a user-driven event, and
 * the fetch belongs there.
 *
 * Honesty rules this component enforces (never relaxed for a nicer demo):
 *   - A `seam_only` mode renders visibly disabled with its stated reason —
 *     never selectable as if it served inference.
 *   - `subscription_cli` NEVER gets a credential field. It's a status row
 *     (signed in / not signed in / unknown) plus an explicit note that
 *     saving it spawns a local process on this machine, gated on consent.
 *   - An `api_key` mode cannot be saved until `POST /api/ai/provider/test`
 *     has returned `ok: true` for the key currently in the box — editing
 *     the key after a successful test invalidates that test.
 *   - If the backend hasn't shipped this endpoint yet (404/405/network),
 *     the dialog says so plainly. It never renders fabricated allowlist
 *     data to look like a working settings page.
 */

type LoadState =
  | { phase: 'loading' }
  | { phase: 'unavailable' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; data: ProviderStatusResponse }

const MODE_LABELS: Record<CredentialMode, string> = {
  api_key: 'API key',
  oauth_profile: 'OAuth profile (CLI login)',
  workload_identity: 'Workload identity',
  subscription_cli: 'Subscription (local CLI)',
}

function wiringBadge(entry: ModeEntry) {
  if (entry.wiring.status === 'wired') {
    return (
      <Badge className="h-4 border-emerald-500/40 bg-emerald-500/10 px-1.5 text-[10px] text-emerald-400">
        wired
      </Badge>
    )
  }
  return (
    <Badge
      variant="secondary"
      className="h-4 border-dashed border-border px-1.5 text-[10px] text-muted-foreground"
      title={entry.wiring.reason}
    >
      not yet wired
    </Badge>
  )
}

export function ProviderSettingsButton() {
  const [open, setOpen] = useState(false)
  const [state, setState] = useState<LoadState>({ phase: 'loading' })
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(null)
  const [selectedMode, setSelectedMode] = useState<CredentialMode | null>(null)
  const [apiKey, setApiKey] = useState('')
  const [testedKey, setTestedKey] = useState<string | null>(null)
  const [testResult, setTestResult] = useState<{ ok: boolean; message?: string } | null>(null)
  const [testing, setTesting] = useState(false)
  const [consent, setConsent] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [clearing, setClearing] = useState(false)

  const load = () => {
    setState({ phase: 'loading' })
    void getProviderStatus().then((res) => {
      if (!res.ok) {
        setState(
          res.kind === 'unavailable'
            ? { phase: 'unavailable' }
            : { phase: 'error', message: res.message },
        )
        return
      }
      setState({ phase: 'ready', data: res.data })
      if (res.data.active) {
        setSelectedProviderId(res.data.active.provider)
        setSelectedMode(res.data.active.mode as CredentialMode)
      }
    })
  }

  /** Opens the dialog and fetches fresh state — invoked from the trigger
   *  button's `onClick`, i.e. a real user event, not an Effect. */
  function openDialog() {
    setOpen(true)
    setApiKey('')
    setTestedKey(null)
    setTestResult(null)
    setConsent(false)
    setSaveError(null)
    load()
  }

  const data = state.phase === 'ready' ? state.data : null
  const selectedProvider: AllowlistedProvider | undefined = data?.allowlist.find(
    (p) => p.id === selectedProviderId,
  )
  const selectedEntry: ModeEntry | undefined = selectedProvider?.modes.find(
    (m) => m.mode === selectedMode,
  )
  const cliDetection: CliDetection | undefined =
    selectedProviderId && data ? data.cli[CLI_KEY_FOR_PROVIDER[selectedProviderId]] : undefined
  const isConfigured =
    data?.active?.provider === selectedProviderId && data?.active?.mode === selectedMode

  async function runTest() {
    if (!selectedProviderId || !selectedMode) return
    setTesting(true)
    setTestResult(null)
    const res = await testProvider({
      provider: selectedProviderId,
      mode: selectedMode,
      api_key: apiKey,
      consent_spawn_local_process: consent,
    })
    setTesting(false)
    if (!res.ok) {
      setTestResult({
        ok: false,
        message:
          res.kind === 'unavailable'
            ? 'Test endpoint not available yet.'
            : [res.message, res.hint].filter(Boolean).join(' — '),
      })
      return
    }
    setTestResult({ ok: res.data.success, message: res.data.success ? 'Verified.' : undefined })
    if (res.data.success) setTestedKey(apiKey)
  }

  async function save() {
    if (!selectedProviderId || !selectedMode) return
    setSaving(true)
    setSaveError(null)
    const res = await putProvider({
      provider: selectedProviderId,
      mode: selectedMode,
      consent_spawn_local_process: consent,
      ...(selectedMode === 'api_key' ? { api_key: apiKey } : {}),
    })
    setSaving(false)
    if (!res.ok) {
      setSaveError(
        res.kind === 'unavailable'
          ? 'Save endpoint not available yet.'
          : [res.message, res.hint].filter(Boolean).join(' — '),
      )
      return
    }
    load()
  }

  async function clear() {
    setClearing(true)
    setSaveError(null)
    const res = await deleteProvider()
    setClearing(false)
    if (!res.ok) {
      setSaveError(res.kind === 'unavailable' ? 'Clear endpoint not available yet.' : res.message)
      return
    }
    setSelectedProviderId(null)
    setSelectedMode(null)
    load()
  }

  const keyTested = selectedMode === 'api_key' && testedKey !== null && testedKey === apiKey && testResult?.ok
  // Known-bad CLI detection blocks Save client-side too — no point letting
  // the user submit a request the backend's own `validate_subscription_cli`
  // will refuse. `cliDetection == null` (not yet known) does NOT block —
  // the backend is still the authority and will refuse it there if wrong.
  const cliDetectionOk =
    selectedMode !== 'subscription_cli' ||
    cliDetection == null ||
    (cliDetection.installed && cliDetection.signed_in)
  const canSave =
    !!selectedProviderId &&
    !!selectedMode &&
    selectedEntry?.wiring.status === 'wired' &&
    !isConfigured &&
    (selectedMode === 'api_key'
      ? !!apiKey && keyTested
      : selectedEntry?.spawns_local_process
        ? consent && cliDetectionOk
        : true)

  // Drives the mandala. Only true once the backend has actually said so —
  // an unreachable or 404 endpoint leaves it false, never optimistic.
  const providerServing = state.phase === 'ready' && state.data.ai_configured

  return (
    <>
      <button
        onClick={openDialog}
        className={cn(
          // Matches FlyoutGroup's trigger exactly so it reads as a rail item
          // rather than a logo dropped into the column.
          'cad-focus w-14 py-2 flex flex-col items-center justify-center rounded-lg transition-colors cursor-pointer gap-1',
          providerServing
            ? 'text-emerald-600 hover:bg-accent dark:text-emerald-400'
            : 'text-muted-foreground hover:text-foreground hover:bg-accent',
        )}
        title={
          providerServing ? 'AI provider — connected' : 'AI provider — not connected yet'
        }
        aria-label={
          providerServing
            ? 'AI provider settings (connected)'
            : 'AI provider settings (not connected)'
        }
      >
        {/* A chip, not a glyph: nothing to interpret, nothing to date. The
            border carries connection state so the mark never claims more
            than the backend has confirmed. */}
        <span
          className={cn(
            'inline-flex h-[22px] min-w-[26px] items-center justify-center rounded',
            'px-1 text-[11px] font-semibold leading-none tracking-wider ring-1',
            providerServing
              ? 'ring-emerald-500/60 bg-emerald-500/10'
              : 'ring-current/40',
          )}
        >
          AI
        </span>
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            AI provider
          </DialogTitle>
          <DialogDescription>
            Choose which LLM provider Roshera talks to. Only server-allowlisted providers and
            modes are ever offered here.
          </DialogDescription>
        </DialogHeader>

        {state.phase === 'loading' && (
          <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <Loader2 size={14} className="animate-spin" /> Loading…
          </div>
        )}

        {state.phase === 'unavailable' && (
          <div className="rounded-md border border-dashed border-border px-3 py-4 text-sm text-muted-foreground">
            Provider settings aren&apos;t available on this backend build yet
            (<code className="cad-readout">GET /api/ai/provider</code> 404/unreachable). Nothing
            is configured from here until that lands.
          </div>
        )}

        {state.phase === 'error' && (
          <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-3 text-sm text-red-400">
            {state.message}
          </div>
        )}

        {data && (
          <div className="flex max-h-[60vh] flex-col gap-4 overflow-y-auto pr-1">
            {data.active && (
              <div className="flex items-center justify-between rounded-md border border-border/60 bg-background/40 px-3 py-2 text-xs">
                <span>
                  Currently configured:{' '}
                  <span className="font-medium text-foreground">
                    {data.active.provider} ·{' '}
                    {MODE_LABELS[data.active.mode as CredentialMode] ?? data.active.mode}
                  </span>{' '}
                  <span className={data.ai_configured ? 'text-emerald-400' : 'text-amber-400'}>
                    ({data.ai_configured ? 'serving' : 'not serving'})
                  </span>
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-6 px-2 text-[11px]"
                  disabled={clearing}
                  onClick={() => void clear()}
                >
                  {clearing ? 'Clearing…' : 'Clear'}
                </Button>
              </div>
            )}

            {data.allowlist.map((provider) => (
              <div key={provider.id}>
                <div className="mb-1 text-xs font-medium text-foreground/90">
                  {provider.display_name}
                </div>
                <div className="space-y-1">
                  {provider.modes.map((entry) => {
                    const active =
                      selectedProviderId === provider.id && selectedMode === entry.mode
                    const disabled = entry.wiring.status !== 'wired'
                    return (
                      <button
                        key={entry.mode}
                        type="button"
                        disabled={disabled}
                        onClick={() => {
                          setSelectedProviderId(provider.id)
                          setSelectedMode(entry.mode)
                          setTestResult(null)
                          setTestedKey(null)
                          setSaveError(null)
                        }}
                        className={cn(
                          'flex w-full flex-col gap-0.5 rounded border px-2 py-1.5 text-left text-xs transition-colors',
                          disabled
                            ? 'cursor-not-allowed border-border/40 opacity-60'
                            : active
                              ? 'border-primary/60 bg-primary/10'
                              : 'border-border hover:bg-accent/30',
                        )}
                      >
                        <span className="flex items-center gap-1.5">
                          <span className="font-medium">{MODE_LABELS[entry.mode]}</span>
                          {wiringBadge(entry)}
                          {entry.spawns_local_process && (
                            <span
                              className="flex items-center gap-0.5 text-[10px] text-amber-400/90"
                              title="Spawns a local process on this machine"
                            >
                              <Terminal size={10} /> local process
                            </span>
                          )}
                        </span>
                        <span className="text-[10px] text-muted-foreground">
                          {entry.wiring.status === 'seam_only' ? entry.wiring.reason : entry.reason}
                        </span>
                      </button>
                    )
                  })}
                </div>
              </div>
            ))}

            {selectedProvider && selectedEntry && selectedEntry.wiring.status === 'wired' && (
              <div className="rounded-md border border-border/60 bg-background/40 p-3">
                {isConfigured && (
                  <p className="mb-2 text-[11px] text-emerald-400">
                    This is the active configuration.
                  </p>
                )}

                {selectedMode === 'api_key' && (
                  <div className="flex flex-col gap-2">
                    <Input
                      type="password"
                      placeholder="API key"
                      value={apiKey}
                      onChange={(e) => {
                        setApiKey(e.target.value)
                        setTestResult(null)
                      }}
                      autoComplete="off"
                    />
                    <div className="flex items-center gap-2">
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-[11px]"
                        disabled={!apiKey || testing}
                        onClick={() => void runTest()}
                      >
                        {testing ? 'Testing…' : 'Test'}
                      </Button>
                      {testResult && (
                        <span
                          className={cn(
                            'flex items-center gap-1 text-[11px]',
                            testResult.ok ? 'text-emerald-400' : 'text-red-400',
                          )}
                        >
                          {testResult.ok ? <CheckCircle2 size={12} /> : <XCircle size={12} />}
                          {testResult.message ?? (testResult.ok ? 'Key works.' : 'Key failed.')}
                        </span>
                      )}
                    </div>
                    <p className="text-[10px] text-muted-foreground">
                      A key must pass a live test before it can be saved — Roshera never claims a
                      key works without checking.
                    </p>
                  </div>
                )}

                {selectedMode === 'subscription_cli' && (
                  <div className="flex flex-col gap-2">
                    <div className="flex items-center gap-1.5 text-xs">
                      {cliDetection == null ? (
                        <span className="text-muted-foreground">
                          Sign-in status unknown until connected to the backend.
                        </span>
                      ) : !cliDetection.installed ? (
                        <span className="flex items-center gap-1 text-amber-400">
                          <XCircle size={12} /> CLI not detected on this machine.
                        </span>
                      ) : cliDetection.signed_in ? (
                        <span className="flex items-center gap-1 text-emerald-400">
                          <CheckCircle2 size={12} /> {selectedProvider.display_name} (Max/Pro
                          subscription) — signed in
                        </span>
                      ) : (
                        <span className="flex items-center gap-1 text-amber-400">
                          <XCircle size={12} /> CLI installed, not signed in — sign in with the CLI
                          first.
                        </span>
                      )}
                    </div>
                    <label className="flex items-start gap-2 text-[11px] text-muted-foreground">
                      <input
                        type="checkbox"
                        className="mt-0.5"
                        checked={consent}
                        onChange={(e) => setConsent(e.target.checked)}
                      />
                      I understand selecting this spawns a local CLI process on this machine and
                      uses my own subscription login — this is only coherent when the backend and
                      this browser share a machine.
                    </label>
                  </div>
                )}

                {selectedMode === 'oauth_profile' && (
                  <p className="text-[11px] text-muted-foreground">
                    Uses a short-lived OAuth token from a CLI login profile already on the
                    backend&apos;s machine. No credential is entered here.
                  </p>
                )}

                {selectedMode === 'workload_identity' && (
                  <p className="text-[11px] text-muted-foreground">
                    Detected from environment variables on the backend&apos;s deployment. Nothing
                    to enter here.
                  </p>
                )}

                {saveError && <p className="mt-2 text-[11px] text-red-400">{saveError}</p>}

                <div className="mt-3 flex justify-end">
                  <Button
                    size="sm"
                    className="h-7 px-3 text-[11px]"
                    disabled={!canSave || saving}
                    onClick={() => void save()}
                  >
                    {saving ? 'Saving…' : isConfigured ? 'Saved' : 'Save'}
                  </Button>
                </div>
              </div>
            )}
          </div>
        )}
      </DialogContent>
      </Dialog>
    </>
  )
}
