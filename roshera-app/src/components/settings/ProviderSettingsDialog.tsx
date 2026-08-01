import { useEffect, useState } from 'react'
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
import { cn } from '@/lib/utils'
import { VendorMark } from '@/components/settings/vendor-marks'
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
 * ## Layout (2026-07-31 redesign)
 * A row of vendor marks is the primary control — you recognise a logo
 * faster than you read a heading. Selecting one swaps a SINGLE options
 * panel underneath for that vendor only (no accordion, no second list of
 * every mode across every vendor). Each mode is one scannable line: a
 * label plus a short status (`✓ signed in`, `needs a key`, `not yet
 * wired`); the old paragraph-length reasons live in `title` tooltips, not
 * as standing prose. There is exactly one model control in the whole
 * dialog — it lives inside this one panel, never duplicated in a second
 * "connected" card, so there is never more than one model input mounted
 * at a time.
 *
 * The fetch-and-reset-on-open logic runs from the trigger button's
 * `onClick` (a real event handler), not a `useEffect` keyed on `open` —
 * `react-hooks/set-state-in-effect` (this project's eslint config)
 * correctly flags synchronous `setState` in an Effect body as a
 * cascading-render smell; opening a dialog is a user-driven event, and
 * the fetch belongs there.
 *
 * Honesty rules this component enforces (never relaxed for a nicer demo):
 *   - A `seam_only` mode renders visibly disabled with its stated reason
 *     on hover — never selectable as if it served inference, and a vendor
 *     logo never implies partnership/endorsement, only identification.
 *   - `subscription_cli` NEVER gets a credential field. It's a status
 *     line (signed in / not signed in / unknown) plus an explicit consent
 *     checkbox — saving it spawns a local process on this machine.
 *   - An `api_key` mode cannot be saved until `POST /api/ai/provider/test`
 *     has returned `ok: true` for the key currently in the box — editing
 *     the key after a successful test invalidates that test.
 *   - The model field is a dropdown of common aliases (`default`, `opus`,
 *     `sonnet`, `haiku`) plus `Custom…` for anything else, always paired
 *     with a line saying these are suggestions, not a verified menu — an
 *     API key and a Max subscription do not necessarily serve the same
 *     models, and the exact name is only confirmed server-side.
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
  oauth_profile: 'OAuth profile',
  workload_identity: 'Workload identity',
  subscription_cli: 'Max/Pro subscription',
}

// Short form for the one-line "connected" readout at the top of the dialog.
const MODE_SHORT_LABELS: Record<CredentialMode, string> = {
  api_key: 'API key',
  oauth_profile: 'CLI profile',
  workload_identity: 'workload identity',
  subscription_cli: 'Max/Pro subscription',
}

// The model field: a dropdown of common Claude aliases plus a free-text
// escape hatch. NOT presented as a verified availability list — which
// models a credential can actually serve isn't knowable in advance (an API
// key and a Max subscription don't necessarily serve the same set), so the
// caption under the control says so explicitly and every value still goes
// through the backend's live validation before it is treated as active.
const CUSTOM_MODEL = '__custom__'
const MODEL_PRESETS: { value: string; label: string }[] = [
  { value: '', label: "default (provider's choice)" },
  { value: 'opus', label: 'opus' },
  { value: 'sonnet', label: 'sonnet' },
  { value: 'haiku', label: 'haiku' },
]

/** One scannable line per mode: a label plus a short, honest status — the
 *  paragraph-length reasons this used to render standing move to `title`
 *  tooltips at the call site instead. */
function modeStatus(
  info: ProviderStatusResponse,
  entry: ModeEntry,
  provider: AllowlistedProvider,
): { text: string; className: string } {
  if (entry.wiring.status !== 'wired') {
    return { text: 'not yet wired', className: 'text-muted-foreground' }
  }
  const isActiveMode =
    info.active?.provider === provider.id && info.active?.mode === entry.mode
  if (entry.mode === 'subscription_cli') {
    const cli = info.cli[CLI_KEY_FOR_PROVIDER[provider.id]]
    if (!cli) return { text: 'status unknown', className: 'text-muted-foreground' }
    if (!cli.installed) return { text: 'CLI not detected', className: 'text-amber-400/90' }
    return cli.signed_in
      ? { text: '✓ signed in', className: 'text-emerald-400' }
      : { text: 'not signed in', className: 'text-amber-400/90' }
  }
  if (isActiveMode) return { text: '✓ configured', className: 'text-emerald-400' }
  if (entry.mode === 'api_key') return { text: 'needs a key', className: 'text-muted-foreground' }
  if (entry.mode === 'oauth_profile') return { text: 'CLI login', className: 'text-muted-foreground' }
  return { text: 'from environment', className: 'text-muted-foreground' }
}

export function ProviderSettingsButton() {
  const [open, setOpen] = useState(false)
  const [state, setState] = useState<LoadState>({ phase: 'loading' })
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(null)
  const [selectedMode, setSelectedMode] = useState<CredentialMode | null>(null)
  const [apiKey, setApiKey] = useState('')
  const [model, setModel] = useState('')
  // Whether the model field is in "Custom…" mode. Tracked separately from
  // `model` itself: selecting "Custom…" with an empty box must keep showing
  // the free-text field, which a derivation from `model === ''` alone can't
  // tell apart from the `default` preset also being empty.
  const [modelCustom, setModelCustom] = useState(false)
  const [testedFor, setTestedFor] = useState<{ apiKey: string; model: string } | null>(null)
  const [testResult, setTestResult] = useState<{ ok: boolean; message?: string } | null>(null)
  const [testing, setTesting] = useState(false)
  const [consent, setConsent] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [clearing, setClearing] = useState(false)

  /** Sets `model` and derives `modelCustom` from whether it matches a
   *  known preset — the single place external data (load, provider switch)
   *  is allowed to set the model field, so the dropdown and the free-text
   *  box never fall out of sync with each other. */
  function applyModel(next: string) {
    setModel(next)
    setModelCustom(!MODEL_PRESETS.some((p) => p.value === next))
    setTestResult(null)
  }

  /** Selects a vendor logo: swaps the options panel to that provider's
   *  modes, defaulting to its first wired mode (falling back to its first
   *  mode at all, so a seam-only-only provider still shows something). */
  function selectProvider(provider: AllowlistedProvider) {
    setSelectedProviderId(provider.id)
    const wired = provider.modes.find((m) => m.wiring.status === 'wired')
    setSelectedMode((wired ?? provider.modes[0])?.mode ?? null)
    setTestResult(null)
    setTestedFor(null)
    setSaveError(null)
  }

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
        applyModel(res.data.active.model ?? '')
      } else if (res.data.allowlist.length > 0) {
        selectProvider(res.data.allowlist[0])
      }
    })
  }

  // The rail chip has to tell the truth before anyone opens the dialog, so
  // status is fetched once on mount. This is the legitimate use of an Effect
  // — fetching on mount — and the setState lands in the promise callback, not
  // synchronously in the Effect body, so it is not the cascading-render smell
  // `react-hooks/set-state-in-effect` exists to catch. A failure stays silent
  // here: the chip simply reads "not connected", and the dialog reports the
  // real reason when opened.
  useEffect(() => {
    void getProviderStatus().then((res) => {
      if (res.ok) setState({ phase: 'ready', data: res.data })
    })
  }, [])

  /** Opens the dialog and fetches fresh state — invoked from the trigger
   *  button's `onClick`, i.e. a real user event, not an Effect. */
  function openDialog() {
    setOpen(true)
    setApiKey('')
    setModel('')
    setModelCustom(false)
    setTestedFor(null)
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
  const activeProviderMeta: AllowlistedProvider | undefined = data?.allowlist.find(
    (p) => p.id === data.active?.provider,
  )
  const isConfigured =
    data?.active?.provider === selectedProviderId &&
    data?.active?.mode === selectedMode &&
    (data?.active?.model ?? '') === model.trim()

  async function runTest() {
    if (!selectedProviderId || !selectedMode) return
    setTesting(true)
    setTestResult(null)
    const trimmedModel = model.trim()
    const res = await testProvider({
      provider: selectedProviderId,
      mode: selectedMode,
      api_key: apiKey,
      model: trimmedModel || undefined,
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
    if (res.data.success) setTestedFor({ apiKey, model: trimmedModel })
  }

  async function save() {
    if (!selectedProviderId || !selectedMode) return
    setSaving(true)
    setSaveError(null)
    const trimmedModel = model.trim()
    const res = await putProvider({
      provider: selectedProviderId,
      mode: selectedMode,
      model: trimmedModel || undefined,
      // Carry the consent the user already gave for this provider+mode. The
      // checkbox resets every time the dialog opens, so without this a
      // model-only change on an ACTIVE subscription posts consent:false and
      // the backend correctly refuses — the button enables, the save looks
      // like it worked, and nothing changes. Only ever true when this exact
      // provider+mode is already the live config; a NEW local-process mode
      // still requires a fresh, explicit tick.
      consent_spawn_local_process: consent || alreadyConsentedToThisMode,
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
    load()
  }

  // Tied to the (apiKey, model) PAIR, not the key alone — testing model A
  // then switching to model B without re-testing must not read as
  // "verified" for a model that was never checked.
  const keyTested =
    selectedMode === 'api_key' &&
    testedFor !== null &&
    testedFor.apiKey === apiKey &&
    testedFor.model === model.trim() &&
    testResult?.ok
  // Known-bad CLI detection blocks Save client-side too — no point letting
  // the user submit a request the backend's own `validate_subscription_cli`
  // will refuse. `cliDetection == null` (not yet known) does NOT block —
  // the backend is still the authority and will refuse it there if wrong.
  const cliDetectionOk =
    selectedMode !== 'subscription_cli' ||
    cliDetection == null ||
    (cliDetection.installed && cliDetection.signed_in)
  // Consent is about spawning a local process on this machine. Once the user
  // has agreed to that for a provider+mode and it is the ACTIVE config, the
  // process is already running with their blessing — changing which model it
  // serves does not re-open that question. Demanding a fresh tick for a
  // model-only change silently disables Save with no visible reason, which is
  // exactly how it read: the dropdown moved, the label flipped to "Save", and
  // the button stayed dead.
  const alreadyConsentedToThisMode =
    data?.active?.provider === selectedProviderId && data?.active?.mode === selectedMode

  const canSave =
    !!selectedProviderId &&
    !!selectedMode &&
    selectedEntry?.wiring.status === 'wired' &&
    !isConfigured &&
    (selectedMode === 'api_key'
      ? !!apiKey && keyTested
      : selectedEntry?.spawns_local_process
        ? (consent || alreadyConsentedToThisMode) && cliDetectionOk
        : true)

  // Drives the chip. `ai_configured` alone is the WRONG signal: it reports
  // whether the /api/ai REST routes can serve, and the subscription-CLI mode
  // deliberately does not serve those (tool_use is not carried over the CLI
  // transport) — so a genuinely connected Max account read as "not connected".
  // A provider the user has actually configured is `active`; that is what the
  // chip reports. Still never optimistic: an unreachable or 404 endpoint
  // leaves both false.
  const providerServing =
    state.phase === 'ready' && (state.data.active !== null || state.data.ai_configured)

  const modelSelectValue = modelCustom ? CUSTOM_MODEL : model
  // Green is reserved for the model that is ACTIVE — currently saved and in
  // use for this exact provider+mode — never for whatever is merely
  // highlighted while browsing. `null` when the panel open right now isn't
  // the active config at all, so nothing in it reads as active.
  const activeModelValue: string | null =
    data?.active?.provider === selectedProviderId && data?.active?.mode === selectedMode
      ? (data.active.model ?? '')
      : null

  return (
    <>
      <button
        onClick={openDialog}
        className={cn(
          // Matches FlyoutGroup's trigger exactly so it reads as a rail item
          // rather than a logo dropped into the column.
          'cad-focus w-14 py-2 flex flex-col items-center justify-center rounded-lg transition-colors cursor-pointer gap-1 hover:bg-accent',
          // Pastel green connected, pastel red not — the chip's ring and fill
          // are currentColor-derived, so one class drives the whole mark.
          providerServing ? 'text-emerald-500' : 'text-rose-400',
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
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            AI provider
          </DialogTitle>
          <DialogDescription>
            Pick a provider by its mark, then choose how Roshera authenticates. Only
            server-allowlisted providers and modes are ever offered here.
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
          <div className="flex max-h-[60vh] flex-col gap-3 overflow-y-auto pr-1">
            {/* Connected readout — one line, never a card of transport
                prose. The interactive controls for changing any of this
                live in the panel below, never duplicated up here. */}
            {data.active && (
              <div className="flex items-center justify-between gap-2 rounded-md border border-emerald-500/40 bg-emerald-500/5 px-3 py-1.5">
                <span className="flex flex-wrap items-center gap-1 text-[11px] text-foreground/90">
                  <VendorMark
                    providerId={data.active.provider}
                    displayName={activeProviderMeta?.display_name ?? data.active.provider}
                    className="h-3.5 w-3.5"
                  />
                  <CheckCircle2 size={12} className="text-emerald-500" />
                  <span className="font-medium">
                    {activeProviderMeta?.display_name ?? data.active.provider}
                  </span>
                  <span className="text-muted-foreground">
                    · {MODE_SHORT_LABELS[data.active.mode as CredentialMode] ?? data.active.mode}
                    {' · model: '}
                    {data.active.model ? data.active.model : 'default'}
                    {data.active.model && data.active.model_verified === false && ' (unverified)'}
                    {data.active.model && data.active.model_verified === true && ' (verified)'}
                  </span>
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-6 shrink-0 px-2 text-[11px]"
                  disabled={clearing}
                  onClick={() => void clear()}
                >
                  {clearing ? 'Disconnecting…' : 'Disconnect'}
                </Button>
              </div>
            )}

            {/* Vendor marks — the primary control, sized to say so: the
                clearly largest thing in the dialog, in their own brand
                colours (vendor-marks.tsx), with real clear-space so a
                12-vendor allowlist just wraps to a second row rather than
                cramming. Recognise the logo, not a heading: the selected
                one lifts and rings at full strength; unselected ones are
                dimmed, never so faint they read as unavailable — only
                `seam_only` (below) gets that treatment. The currently
                active vendor carries a small dot regardless of selection. */}
            <div className="flex flex-wrap items-start gap-2.5">
              {data.allowlist.map((provider) => {
                const isSelected = provider.id === selectedProviderId
                const isActive = provider.id === data.active?.provider
                return (
                  <button
                    key={provider.id}
                    type="button"
                    onClick={() => selectProvider(provider)}
                    title={provider.display_name}
                    aria-label={provider.display_name}
                    aria-pressed={isSelected}
                    className={cn(
                      'relative flex h-14 w-14 shrink-0 items-center justify-center rounded-xl border-2 transition-all',
                      isSelected
                        ? '-translate-y-0.5 border-primary/70 bg-primary/10 opacity-100 shadow-md'
                        : 'border-border/50 opacity-70 hover:opacity-100 hover:bg-accent/30',
                    )}
                  >
                    <VendorMark
                      providerId={provider.id}
                      displayName={provider.display_name}
                      className="h-7 w-7"
                    />
                    {isActive && (
                      <span className="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full bg-emerald-500 ring-2 ring-background" />
                    )}
                  </button>
                )
              })}
            </div>

            {/* Selected vendor's options — one vendor at a time, no
                accordion. Each mode is one scannable line. */}
            {selectedProvider && (
              <div className="rounded-md border border-border/60 bg-background/40 p-3">
                <div className="mb-2 space-y-1">
                  {selectedProvider.modes.map((entry) => {
                    const active = selectedMode === entry.mode
                    const disabled = entry.wiring.status !== 'wired'
                    const status = modeStatus(data, entry, selectedProvider)
                    return (
                      <button
                        key={entry.mode}
                        type="button"
                        disabled={disabled}
                        onClick={() => {
                          setSelectedMode(entry.mode)
                          setTestResult(null)
                          setTestedFor(null)
                          setSaveError(null)
                        }}
                        title={entry.wiring.status !== 'wired' ? entry.wiring.reason : entry.reason}
                        className={cn(
                          'flex w-full items-center justify-between gap-2 rounded border px-2 py-1.5 text-left text-xs transition-colors',
                          disabled
                            ? 'cursor-not-allowed border-dashed border-border/40 opacity-60'
                            : active
                              ? 'border-primary/60 bg-primary/10'
                              : 'border-border hover:bg-accent/30',
                        )}
                      >
                        <span className="flex items-center gap-1.5 font-medium">
                          {MODE_LABELS[entry.mode]}
                          {entry.spawns_local_process && (
                            <Terminal
                              size={10}
                              className="text-amber-400/90"
                              aria-label="Spawns a local process on this machine"
                            />
                          )}
                        </span>
                        <span className={cn('shrink-0', status.className)}>{status.text}</span>
                      </button>
                    )
                  })}
                </div>

                {selectedEntry && selectedEntry.wiring.status === 'wired' && (
                  <div className="flex flex-col gap-2 border-t border-border/40 pt-2">
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
                          className="h-7 text-[11px]"
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
                        <p
                          className="text-[10px] text-muted-foreground"
                          title="Roshera never claims a key works without checking it live first."
                        >
                          Validated live before it can be saved.
                        </p>
                      </div>
                    )}

                    {selectedMode === 'subscription_cli' && (
                      <label
                        className="flex items-start gap-2 text-[11px] text-muted-foreground"
                        title="Spawns a local CLI process on this machine and uses your own subscription login — only coherent when the backend and this browser share a machine."
                      >
                        <input
                          type="checkbox"
                          className="mt-0.5"
                          checked={consent}
                          onChange={(e) => setConsent(e.target.checked)}
                        />
                        I understand this spawns a local CLI process on this machine
                      </label>
                    )}

                    {selectedMode === 'oauth_profile' && (
                      <p className="text-[11px] text-muted-foreground">
                        Uses an existing CLI login profile on the backend&apos;s machine — no
                        credential entered here.
                      </p>
                    )}

                    {selectedMode === 'workload_identity' && (
                      <p className="text-[11px] text-muted-foreground">
                        Detected from environment variables on the backend&apos;s deployment —
                        nothing to enter here.
                      </p>
                    )}

                    {/* Model — the ONE model control in this dialog, and a
                        plain, unambiguous picker: no colour, no inline
                        per-option styling, nothing that could read as
                        "locked" or already-decided. CHOOSING a model
                        (this dropdown) and what is actually ACTIVE (the
                        green line below) are two different facts — until
                        Save is pressed they can legitimately differ, and
                        the honest UI shows that instead of collapsing them
                        into one. Suggestions, not a verified menu: an API
                        key and a Max subscription don't necessarily serve
                        the same models, so a chosen value is still checked
                        live (except subscription_cli, which has no
                        side-effect-free way to check before session start
                        — the caption below says so plainly). */}
                    <div className="flex flex-col gap-1">
                      <label
                        htmlFor="provider-model-select"
                        className="text-[10px] font-medium text-muted-foreground"
                      >
                        Model
                      </label>
                      <select
                        id="provider-model-select"
                        value={modelSelectValue}
                        onChange={(e) => {
                          const v = e.target.value
                          if (v === CUSTOM_MODEL) {
                            setModelCustom(true)
                            setTestResult(null)
                            return
                          }
                          setModelCustom(false)
                          setModel(v)
                          setTestResult(null)
                        }}
                        className="cad-focus h-7 rounded border border-border/60 bg-background/40 px-1.5 text-[11px] text-foreground/90 hover:bg-accent/30"
                      >
                        {MODEL_PRESETS.map((p) => (
                          <option key={p.value || 'default'} value={p.value}>
                            {p.label}
                          </option>
                        ))}
                        <option value={CUSTOM_MODEL}>Custom…</option>
                      </select>
                      {/* What is actually saved and in use for THIS
                          provider+mode — independent of whatever the
                          dropdown above currently shows. Absent entirely
                          when this panel isn't the active configuration at
                          all (nothing here is running, so no green claim
                          to make). */}
                      {activeModelValue !== null && (
                        <p className="text-[11px] font-medium text-emerald-500">
                          Active: {activeModelValue ? activeModelValue : "provider's default"}
                          {activeModelValue &&
                            selectedMode === 'subscription_cli' &&
                            data.active?.model_verified === false && (
                              <span className="ml-1 font-normal text-emerald-500/80">
                                (unverified — not checked without spawning the CLI)
                              </span>
                            )}
                        </p>
                      )}
                      {modelCustom && (
                        <Input
                          value={model}
                          onChange={(e) => {
                            setModel(e.target.value)
                            setTestResult(null)
                          }}
                          placeholder="exact model id"
                          className="h-7 text-[11px]"
                          autoComplete="off"
                        />
                      )}
                      <p className="text-[10px] text-muted-foreground">
                        {selectedMode === 'subscription_cli'
                          ? 'Suggestions — availability depends on your plan; applied at session start, not checked here.'
                          : 'Suggestions — availability depends on your plan; the exact name is validated on save.'}
                      </p>
                    </div>

                    {saveError && <p className="text-[11px] text-red-400">{saveError}</p>}

                    <div className="flex justify-end">
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
          </div>
        )}
      </DialogContent>
      </Dialog>
    </>
  )
}
