import { useEffect, useRef, useState } from 'react'
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
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { Claim } from '@/components/panels/cards/card-chrome'
import { VendorMark } from '@/components/settings/vendor-marks'
import {
  deleteProvider,
  discoverProviderModels,
  getProviderStatus,
  implausibleApiKeyReason,
  putProvider,
  testProvider,
  type AllowlistedProvider,
  type CliDetection,
  type CredentialMode,
  type DiscoveredModel,
  type ModeEntry,
  type ProviderStatusResponse,
} from '@/lib/provider-api'
import {
  describeAcpConnectFailure,
  establishAcpSession,
  resetAcpClient,
} from '@/lib/acp-blackboard'
import { useAcpSessionStore } from '@/stores/acp-session-store'

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
 * ## Layout (2026-07-31 redesign, decrowded 2026-08-04)
 * A row of vendor marks is the primary control — you recognise a logo
 * faster than you read a heading. Selecting one swaps a SINGLE options
 * panel underneath for that vendor only (no accordion, no second list of
 * every mode across every vendor). Each mode is one scannable line: a
 * label plus a short status (`✓ signed in`, `needs a key`, `not yet
 * wired`); the paragraph-length reasons (a `seam_only` mode's explanation,
 * a vendor mark's Active/Ready/Available/Unavailable state) live in
 * `@/components/ui/tooltip` (Base UI), never as standing prose — that
 * primitive opens on hover AND on keyboard focus (`Tooltip.Trigger`'s
 * `onFocus` handler), unlike a bare `title` attribute, which several rows
 * here used to rely on despite being unreachable by keyboard. There is
 * exactly one model control in the whole dialog — it lives inside this one
 * panel, never duplicated in a second "connected" card, so there is never
 * more than one model input mounted at a time.
 *
 * Two more things Varun asked to stop being permanent (2026-08-04, "pop
 * feels too crowded"): the connect-flow stage list (`ConnectFlow` below)
 * unmounts the instant a run reaches `ready` — the header card's own
 * "agent running" clause already carries that outcome — but stays mounted
 * on `failed`, because that is exactly when the failing stage and its
 * reason are needed and a collapse would hide the only diagnostic. And the
 * agent-surface `Claim` row only renders when `data.active === null`;
 * once a provider is pinned, the header card above already states the
 * pinned provider/mode and whether the session is live, so repeating it
 * in a second row would be pure restatement.
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
 *     in a tooltip (hover or keyboard focus — `aria-disabled`, not the
 *     native `disabled` attribute, so the row stays focusable enough to
 *     open it) — never selectable as if it served inference, and a vendor
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
 *   - **Live model discovery** (`POST /api/ai/provider/models`, `api_key`
 *     mode only): an explicit "Discover models" button — not on-blur,
 *     which would spam the vendor on every focus change — asks the
 *     vendor's own `GET {base_url}/models` what it actually serves. A
 *     successful discovery replaces the preset dropdown with a picker
 *     over the REAL returned list (never merged with the presets) and is
 *     the one place this dialog shows a genuinely earned "Verified"
 *     badge for these vendors — unlike `Test`, which for every non-
 *     anthropic `api_key` vendor accepts the key without a network
 *     round-trip (no vetted synchronous credential-check client exists
 *     for them; see `ai_provider.rs::validate_api_key`'s doc) and so
 *     cannot honestly claim "verified" on its own. A discovery FAILURE
 *     is shown with the vendor's own status/message and blocks Save for
 *     the key currently in the box, even if an earlier `Test` passed.
 *     Before either action fires, the key is checked against
 *     `implausibleApiKeyReason` — client-side pre-flight for the same
 *     shape check the backend enforces, closing the gap that once let a
 *     649-character multi-line Vite error message get saved as a
 *     credential.
 *   - If the backend hasn't shipped this endpoint yet (404/405/network),
 *     the dialog says so plainly. It never renders fabricated allowlist
 *     data to look like a working settings page.
 */

type LoadState =
  | { phase: 'loading' }
  | { phase: 'unavailable' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; data: ProviderStatusResponse }

/**
 * CONNECT FLOW — the real stages between "saved a provider" and "the agent
 * is actually running", per Varun's words: "setting up and connecting
 * harness, checking ai provider". Every stage below is a real awaited
 * operation, never a timed fake:
 *   - `saving`   — `PUT /api/ai/provider` (only present when this run
 *                  followed a Save click; a re-establish-only run has no
 *                  PUT to perform, so it never appears — a stage list must
 *                  never narrate work it isn't doing).
 *   - `checking` — `GET /api/ai/provider` re-fetched, and its `active`
 *                  compared against what was submitted (or, for a
 *                  re-establish, simply confirmed non-null) — catches a 2xx
 *                  PUT that didn't actually pin, not just decoration.
 *   - `starting` — `establishAcpSession()` (`initialize()` + `session/new`
 *                  over `/acp`, WITHOUT sending a turn) — the step that was
 *                  missing entirely before this change, leaving a green
 *                  chip over an agent that only started on the user's first
 *                  Blackboard message.
 */
type ConnectStepId = 'saving' | 'checking' | 'starting'

const CONNECT_STEP_LABELS: Record<ConnectStepId, string> = {
  saving: 'Saving the provider selection',
  checking: 'Checking the provider',
  starting: 'Starting the agent harness',
}

interface ConnectFlow {
  /** Fixed at the start of the run — only the stages THIS run performs. */
  steps: ConnectStepId[]
  status: 'running' | 'ready' | 'failed'
  /** Index into `steps` currently in flight. Meaningless once `status` is
   *  `'ready'` or `'failed'` (the per-step render derives from `status` +
   *  `failure` instead). */
  current: number
  failure?: { step: ConnectStepId; message: string }
}

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

/** The four states Varun asked to make visually distinct — computed only
 *  from facts the backend actually reports (`wiring.status`, `active`,
 *  `cli.*.installed/signed_in`), never invented:
 *    - `active`      — this exact provider+mode is what's currently serving.
 *    - `ready`       — wired, and a credential is ALREADY present without
 *                      the user typing anything (a signed-in local CLI).
 *    - `available`   — wired, but nothing usable has been supplied yet
 *                      (needs a typed key, or the CLI isn't signed in).
 *    - `unavailable` — `seam_only`; never selectable, reason always shown.
 *  `ready` is honestly narrow: this server stores exactly one credential
 *  at a time (the active one — `ai_provider.rs::get_provider`'s `stored`
 *  is a single `Option`), so there is no "a key is stored for this OTHER
 *  mode" fact to report. `ready` only fires for CLI-detected modes, where
 *  "a credential exists" is a live fact (`cli.installed && cli.signed_in`)
 *  independent of what Roshera has persisted. */
type WireState = 'active' | 'ready' | 'available' | 'unavailable'

// Colour is the ONLY thing that varies across these four (Varun's
// standing rule: colour carries state, nothing else) — the dot shape is
// identical everywhere, only its fill changes. `active` is reserved for
// this bucket ONLY; nothing else in the dialog may reuse emerald.
const STATE_STYLES: Record<WireState, { text: string; dot: string }> = {
  active: { text: 'text-emerald-400', dot: 'bg-emerald-500' },
  ready: { text: 'text-sky-400', dot: 'bg-sky-400' },
  available: { text: 'text-amber-400/90', dot: 'bg-amber-400' },
  unavailable: { text: 'text-muted-foreground', dot: 'bg-muted-foreground/40' },
}

const STATE_LABELS: Record<WireState, string> = {
  active: 'Active',
  ready: 'Ready',
  available: 'Available',
  unavailable: 'Unavailable',
}

function modeWireState(
  info: ProviderStatusResponse,
  entry: ModeEntry,
  provider: AllowlistedProvider,
): WireState {
  if (entry.wiring.status !== 'wired') return 'unavailable'
  if (info.active?.provider === provider.id && info.active?.mode === entry.mode) return 'active'
  if (entry.mode === 'subscription_cli' || entry.mode === 'oauth_profile') {
    const cli = info.cli[CLI_KEY_FOR_PROVIDER[provider.id]]
    return cli?.installed && cli?.signed_in ? 'ready' : 'available'
  }
  return 'available'
}

/** Aggregate state for the vendor mark itself — one dot has to summarize
 *  every mode underneath it. `ready` beats `available` beats
 *  `unavailable` so a vendor with one usable mode never reads as fully
 *  unavailable. */
function providerWireState(info: ProviderStatusResponse, provider: AllowlistedProvider): WireState {
  if (info.active?.provider === provider.id) return 'active'
  const states = provider.modes.map((m) => modeWireState(info, m, provider))
  if (states.includes('ready')) return 'ready'
  if (states.includes('available')) return 'available'
  return 'unavailable'
}

/** One scannable line per mode: a label plus a short, honest status — the
 *  paragraph-length reasons this used to render standing move to `title`
 *  tooltips (and, for `unavailable`, a visible line under the row) at the
 *  call site instead. */
function modeStatus(
  info: ProviderStatusResponse,
  entry: ModeEntry,
  provider: AllowlistedProvider,
): { text: string; state: WireState } {
  const state = modeWireState(info, entry, provider)
  if (state === 'unavailable') return { text: 'not yet wired', state }
  if (state === 'active') return { text: '✓ configured', state }
  if (entry.mode === 'subscription_cli') {
    const cli = info.cli[CLI_KEY_FOR_PROVIDER[provider.id]]
    if (!cli) return { text: 'status unknown', state: 'available' }
    if (!cli.installed) return { text: 'CLI not detected', state }
    return cli.signed_in ? { text: 'signed in — ready', state } : { text: 'not signed in', state }
  }
  if (entry.mode === 'oauth_profile') {
    return state === 'ready'
      ? { text: 'CLI login — ready', state }
      : { text: 'CLI login required', state }
  }
  if (entry.mode === 'api_key') return { text: 'needs a key', state }
  return { text: 'from environment', state }
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
  // Live model discovery (`POST /api/ai/provider/models`) — a separate
  // action from `Test` above. Keyed to the exact `apiKey` it ran against
  // (mirrors `testedFor`'s pairing) so editing the key after a discovery
  // — success OR failure — invalidates it rather than leaving a stale
  // model list or a stale block on Save.
  const [discovery, setDiscovery] = useState<
    | { phase: 'discovering'; apiKey: string }
    | { phase: 'success'; apiKey: string; models: DiscoveredModel[]; baseUrl: string }
    | { phase: 'error'; apiKey: string; message: string }
    | null
  >(null)
  const [consent, setConsent] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [clearing, setClearing] = useState(false)
  const [connectFlow, setConnectFlow] = useState<ConnectFlow | null>(null)
  // Bumped at the start of every `runConnect` call (and on dialog
  // open/close) so a slow, superseded run's `setConnectFlow` calls are
  // dropped instead of clobbering a newer run or a closed dialog — the
  // same stale-result discipline `runDiscovery` already applies via
  // `keyAtRequestTime`.
  const flowTokenRef = useRef(0)
  // The one fact the chip and this dialog now agree on: a genuinely live
  // `/acp` session, not "a provider is configured" (see `providerServing`'s
  // doc below for why that used to be wrong).
  const acpLive = useAcpSessionStore((s) => s.live)

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
    setDiscovery(null)
    setSaveError(null)
  }

  /** `autoStart`: after a fresh load shows an already-configured provider
   *  with no live agent session, kick off the connect flow's `checking` +
   *  `starting` stages automatically — Varun: "clicking ai should start
   *  it". Only fires when `active !== null` (nothing to confirm otherwise)
   *  and only from a real click handler (`openDialog`), never an Effect.
   *  Not a consent bypass: for an ALREADY-active provider the user already
   *  ticked the local-process consent box when it was saved (same
   *  reasoning `alreadyConsentedToThisMode` below encodes) — this only
   *  resumes a harness for a config the user already approved, it never
   *  saves a new one. */
  const load = (opts?: { autoStart?: boolean }) => {
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
      if (opts?.autoStart && res.data.active && !useAcpSessionStore.getState().live) {
        void runConnect({
          save: false,
          provider: res.data.active.provider,
          mode: res.data.active.mode as CredentialMode,
        })
      }
    })
  }

  /**
   * Runs the real, awaited stage sequence behind the connect flow UI:
   *   `save` (only when `opts.save`) → `checking` → `starting`.
   * `opts.provider`/`opts.mode` let a caller target the ALREADY-active
   * config (the "Start agent" button, and `autoStart` above) without
   * depending on whatever the vendor panel currently has selected — those
   * two can legitimately differ. Falls back to the panel's own selection
   * for the ordinary Save-button path.
   */
  async function runConnect(opts: {
    save: boolean
    provider?: string
    mode?: CredentialMode
  }): Promise<void> {
    const submittedProvider = opts.provider ?? selectedProviderId
    const submittedMode = opts.mode ?? selectedMode
    if (!submittedProvider || !submittedMode) return

    const token = ++flowTokenRef.current
    const isCurrent = () => flowTokenRef.current === token
    const steps: ConnectStepId[] = opts.save ? ['saving', 'checking', 'starting'] : ['checking', 'starting']
    const fail = (step: ConnectStepId, message: string) => {
      if (!isCurrent()) return
      setConnectFlow({ steps, status: 'failed', current: steps.indexOf(step), failure: { step, message } })
    }

    setConnectFlow({ steps, status: 'running', current: 0 })

    if (opts.save) {
      setSaving(true)
      setSaveError(null)
      const trimmedModel = model.trim()
      const res = await putProvider({
        provider: submittedProvider,
        mode: submittedMode,
        model: trimmedModel || undefined,
        // See `alreadyConsentedToThisMode`'s doc: only ever true when this
        // exact provider+mode is already the live config.
        consent_spawn_local_process: consent || alreadyConsentedToThisMode,
        ...(submittedMode === 'api_key' ? { api_key: apiKey } : {}),
      })
      setSaving(false)
      if (!isCurrent()) return
      if (!res.ok) {
        // Named on the stage row itself (`fail` below) — not duplicated
        // into `saveError` too, which would print the identical sentence
        // twice on screen (the row already carries the stage AND the
        // reason, which is strictly more informative than the bare
        // message `saveError` would show on its own).
        fail(
          'saving',
          res.kind === 'unavailable'
            ? 'Save endpoint not available yet.'
            : [res.message, res.hint].filter(Boolean).join(' — '),
        )
        return
      }
      // The backend just ended every connection minted under the OLD
      // provider pin (`acp_provider_epoch.rs` — the same mechanism
      // `acp-client.ts`'s `reestablish()` documents for a repin). Without
      // discarding the shared client here, the `starting` stage below
      // would call `getAcpClient()`, get back the pre-save client (its
      // `isDead` flag hasn't caught up to the backend yet), and tick
      // "ready" over a connection the backend has already killed — the
      // exact defect this task removes, recreated one layer up.
      resetAcpClient()
    }

    setConnectFlow({ steps, status: 'running', current: steps.indexOf('checking') })
    const statusRes = await getProviderStatus()
    if (!isCurrent()) return
    if (!statusRes.ok) {
      fail(
        'checking',
        statusRes.kind === 'unavailable'
          ? 'Provider status endpoint not available yet.'
          : [statusRes.message, statusRes.hint].filter(Boolean).join(' — '),
      )
      return
    }
    setState({ phase: 'ready', data: statusRes.data })
    const active = statusRes.data.active
    if (!active) {
      fail('checking', 'No provider is pinned on the backend — pick one and Save before connecting.')
      return
    }
    if (active.provider !== submittedProvider || active.mode !== submittedMode) {
      fail(
        'checking',
        `The backend is pinned to ${active.provider}/${active.mode}, not ` +
          `${submittedProvider}/${submittedMode} — the save did not take effect as expected.`,
      )
      return
    }
    // Sync the panel to what the backend actually confirmed — the same
    // resync `load()` always did after a save, now shared with the
    // re-establish-only path too.
    setSelectedProviderId(active.provider)
    setSelectedMode(active.mode as CredentialMode)
    applyModel(active.model ?? '')

    setConnectFlow({ steps, status: 'running', current: steps.indexOf('starting') })
    try {
      await establishAcpSession()
    } catch (err) {
      if (!isCurrent()) return
      fail('starting', describeAcpConnectFailure(err, 'dialog'))
      return
    }
    if (!isCurrent()) return
    setConnectFlow({ steps, status: 'ready', current: steps.length })
  }

  // The chip itself is driven by `acpLive` (the real session store), not by
  // this fetch — see `providerServing`'s doc below. This mount fetch exists
  // so the dialog's OWN content (the vendor grid, the "pinned to X" line,
  // mode statuses) isn't a blank loading spinner for a beat the very first
  // time the dialog opens. This is the legitimate use of an Effect —
  // fetching on mount — and the setState lands in the promise callback, not
  // synchronously in the Effect body, so it is not the cascading-render
  // smell `react-hooks/set-state-in-effect` exists to catch. A failure
  // stays silent here: the dialog just shows its own loading/empty state
  // when actually opened, which fetches again anyway.
  useEffect(() => {
    void getProviderStatus().then((res) => {
      if (res.ok) setState({ phase: 'ready', data: res.data })
    })
  }, [])

  /** Opens the dialog and fetches fresh state — invoked from the trigger
   *  button's `onClick`, i.e. a real user event, not an Effect.
   *  `autoStart: true` is Varun's "clicking ai should start it": if the
   *  fresh load shows an already-configured provider with no live agent
   *  session, `load()` itself kicks off the checking+starting stages. */
  function openDialog() {
    setOpen(true)
    setApiKey('')
    setModel('')
    setModelCustom(false)
    setTestedFor(null)
    setTestResult(null)
    setDiscovery(null)
    setConsent(false)
    setSaveError(null)
    // Invalidate any in-flight connect flow from a previous open so its
    // late `setConnectFlow` calls can't land on this fresh dialog state.
    flowTokenRef.current++
    setConnectFlow(null)
    load({ autoStart: true })
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

  // No `load()` after this call: `POST /api/ai/provider/test` shares
  // `PUT`'s validation path but deliberately "stops before any of that"
  // (`ai_provider.rs`'s own doc comment) — no persist, no repin, no env
  // scrub. Nothing on the server changed, so there is nothing for a
  // refetch to pick up; it would only flash `phase: 'loading'` and
  // unmount this panel for no reason. `setTestResult`/`setTesting` below
  // are this action's own visible pending/outcome state.
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

  // Same "no refetch" reasoning as `runTest` above: discovery is a pure
  // lookup, nothing on the server changes, so there is nothing for
  // `load()` to pick up. Guarded by `implausibleApiKeyReason` before the
  // network call — belt-and-suspenders with the backend's own
  // `reject_implausible_key_shape`, which is the actual enforcement
  // point.
  async function runDiscovery() {
    if (!selectedProviderId || selectedMode !== 'api_key') return
    const shapeError = implausibleApiKeyReason(apiKey)
    if (shapeError) {
      setDiscovery({ phase: 'error', apiKey, message: `API key ${shapeError}` })
      return
    }
    const keyAtRequestTime = apiKey
    setDiscovery({ phase: 'discovering', apiKey: keyAtRequestTime })
    const res = await discoverProviderModels({ provider: selectedProviderId, api_key: apiKey })
    // The key may have changed while the request was in flight — a
    // result for a superseded key must never land as if it were current
    // (same stale-result discipline as the backend's own
    // `AiProviderManager::update_model_verification`).
    if (apiKey !== keyAtRequestTime) return
    if (!res.ok) {
      setDiscovery({
        phase: 'error',
        apiKey: keyAtRequestTime,
        message:
          res.kind === 'unavailable'
            ? 'Model discovery endpoint not available yet.'
            : [res.message, res.hint].filter(Boolean).join(' — '),
      })
      return
    }
    setDiscovery({
      phase: 'success',
      apiKey: keyAtRequestTime,
      models: res.data.models,
      baseUrl: res.data.base_url,
    })
    // A successful discovery is real evidence the key works — apply it
    // to the model field too, exactly the way a preset click does, so
    // the picker below shows a value that is actually in the returned
    // list rather than an empty preset.
    if (res.data.models.length > 0 && !res.data.models.some((m) => m.id === model)) {
      applyModel(res.data.models[0].id)
    }
  }

  async function clear() {
    setClearing(true)
    setSaveError(null)
    // Invalidate any in-flight connect run — the provider it was starting
    // a harness for is about to stop being the saved config.
    flowTokenRef.current++
    setConnectFlow(null)
    const res = await deleteProvider()
    setClearing(false)
    if (!res.ok) {
      setSaveError(res.kind === 'unavailable' ? 'Clear endpoint not available yet.' : res.message)
      return
    }
    // The saved config this session was pinned to is gone — a harness left
    // running against it would be a live agent session with no
    // corresponding "connected" claim anywhere in the UI. Tear it down so
    // the chip honestly drops to red along with the config.
    resetAcpClient()
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

  // A discovery FAILURE for the exact key currently in the box is
  // known-bad evidence and must block Save outright — independent of
  // `keyTested`, which for every non-anthropic api_key vendor accepts
  // the key without a network round-trip and would otherwise still read
  // "tested" for a key discovery just proved doesn't work.
  const discoveryFailedForCurrentKey =
    discovery?.phase === 'error' && discovery.apiKey === apiKey
  // A discovery SUCCESS is an alternative, stronger gate than `keyTested`
  // — it is a real vendor round-trip, unlike `Test` for these vendors
  // (see this component's module doc). Without this, picking a model out
  // of the very list discovery just returned moves `model` away from
  // whatever `testedFor.model` recorded and kills `keyTested`, disabling
  // Save right after the dialog showed a "Verified" badge — the same
  // dead-button-with-no-visible-reason bug `alreadyConsentedToThisMode`
  // above exists to prevent for the consent checkbox.
  const discoveryVerifiedForCurrentKey =
    discovery?.phase === 'success' && discovery.apiKey === apiKey

  const canSave =
    !!selectedProviderId &&
    !!selectedMode &&
    selectedEntry?.wiring.status === 'wired' &&
    !isConfigured &&
    connectFlow?.status !== 'running' &&
    (selectedMode === 'api_key'
      ? !!apiKey &&
        (keyTested || discoveryVerifiedForCurrentKey) &&
        !discoveryFailedForCurrentKey
      : selectedEntry?.spawns_local_process
        ? (consent || alreadyConsentedToThisMode) && cliDetectionOk
        : true)

  // Drives the chip. Config alone (`active !== null` / `ai_configured`) used
  // to be the signal — and that was the bug: it reports whether a provider
  // is SAVED, not whether the agent is RUNNING. goose only actually starts
  // when `getAcpClient()`/`establishAcpSession()` completes an
  // `initialize()` + `session/new` round-trip, which used to happen
  // invisibly on the user's first Blackboard message — so "connect a
  // provider" lit the chip green over an agent that had not started yet.
  // `acpLive` (`useAcpSessionStore`) is the one fact that's actually true
  // only while a live `/acp` session exists: set by `startSession` when
  // `establishAcpSession()`/the first turn succeeds, cleared by
  // `endSession` the moment the session drops (`AcpClient.onDisconnect` —
  // backend restart, SSE inactivity watchdog, an explicit
  // `resetAcpClient()`) — see `acp-session-store.ts`'s module doc. Because
  // this is a live store subscription (`useAcpSessionStore((s) => s.live)`
  // above), the chip re-renders the instant a session drops, never staying
  // green on a stale success.
  const providerServing = acpLive

  // Non-null only when discovery succeeded for the exact key currently in
  // the box — the model field becomes a picker over this REAL list
  // (never merged with `MODEL_PRESETS`) instead of the preset+free-text
  // control. An empty array (vendor served zero models) is still an
  // honest "success" and still suppresses the preset/free-text UI —
  // showing stale presets after a real, empty answer would misrepresent
  // what was actually discovered.
  const discoveredModels: DiscoveredModel[] | null =
    discovery?.phase === 'success' && discovery.apiKey === apiKey ? discovery.models : null

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
          /* `pt-2` is load-bearing, not spacing. Each vendor mark's state
             dot is positioned `-top-1 -right-1` — deliberately overhanging
             its button — and this element scrolls, so anything outside the
             scroll box is clipped at its edge and the top row's dots lose
             their upper sliver. The right side was already compensated with
             `pr-1`; the top was missed.
             The selected mark also carries `-translate-y-0.5`, lifting it a
             further 2px, so the provider actually picked is the one that
             clips worst — 4px of offset plus a 2px ring plus 2px of lift is
             why this is `pt-2` and not `pt-1`. Padding sits INSIDE the
             scroll box, so it gives the overhang somewhere to live without
             shifting the content. */
          <div className="flex max-h-[60vh] flex-col gap-3 overflow-y-auto pr-1 pt-2">
            {/* Connected readout — one line, never a card of transport
                prose. The interactive controls for changing any of this
                live in the panel below, never duplicated up here.
                Colour + the "agent running"/"agent not started" clause are
                driven by `acpLive`, NOT by `data.active` alone — a saved
                provider with no live session is a real, distinct state
                (amber, not emerald) that needs its own control to resolve:
                "Start agent" below, not just "Disconnect". */}
            {data.active && (
              <div
                className={cn(
                  'flex items-center justify-between gap-2 rounded-md border px-3 py-1.5',
                  acpLive
                    ? 'border-emerald-500/40 bg-emerald-500/5'
                    : 'border-amber-500/40 bg-amber-500/5',
                )}
              >
                <span className="flex flex-wrap items-center gap-1 text-[11px] text-foreground/90">
                  <VendorMark
                    providerId={data.active.provider}
                    displayName={activeProviderMeta?.display_name ?? data.active.provider}
                    className="h-3.5 w-3.5"
                  />
                  {acpLive ? (
                    <CheckCircle2 size={12} className="text-emerald-500" />
                  ) : (
                    <XCircle size={12} className="text-amber-400" />
                  )}
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
                  <span className={acpLive ? 'text-emerald-500' : 'text-amber-400/90'}>
                    · {acpLive ? 'agent running' : 'agent not started'}
                  </span>
                </span>
                <div className="flex shrink-0 items-center gap-1.5">
                  {!acpLive && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-6 px-2 text-[11px]"
                      disabled={connectFlow?.status === 'running'}
                      onClick={() =>
                        void runConnect({
                          save: false,
                          provider: data.active!.provider,
                          mode: data.active!.mode as CredentialMode,
                        })
                      }
                    >
                      {connectFlow?.status === 'running' ? 'Starting…' : 'Start agent'}
                    </Button>
                  )}
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-6 px-2 text-[11px]"
                    disabled={clearing}
                    onClick={() => void clear()}
                  >
                    {clearing ? 'Disconnecting…' : 'Disconnect'}
                  </Button>
                </div>
              </div>
            )}

            {/* The connect flow's real stage list — transient by design
                (Varun: "pop feels too crowded"). It is mounted while a run
                is in progress, and on FAILURE it stays mounted with the
                failing stage and its reason fully visible — that is
                precisely when the detail is needed, and collapsing it would
                hide the only diagnostic. Once a run reaches `ready`, the
                stage list unmounts: the header card's `agent
                running`/`agent not started` clause above already carries
                that outcome, so a three-second-old stage list would only be
                a permanent shelf for a transient event. Rows are whatever
                `runConnect` set as `steps` for THIS run: a re-establish-only
                run never shows a "saving" row it isn't performing. */}
            {connectFlow && connectFlow.status !== 'ready' && (
              <div className="flex flex-col gap-1.5 rounded-md border border-border/60 bg-background/40 px-3 py-2">
                {connectFlow.steps.map((step, i) => {
                  const failedHere = connectFlow.status === 'failed' && connectFlow.failure?.step === step
                  const isCurrent = connectFlow.status === 'running' && connectFlow.current === i
                  const pastThisStep =
                    connectFlow.status === 'failed'
                      ? i < connectFlow.steps.indexOf(connectFlow.failure!.step)
                      : connectFlow.status === 'ready' || i < connectFlow.current
                  // The harness-start row is tied to the SAME live fact the
                  // chip reads, not to "the promise resolved" — if the
                  // session drops right after connect, this row un-ticks
                  // instead of leaving a stale check above a red chip.
                  const done = step === 'starting' && connectFlow.status === 'ready' ? acpLive : pastThisStep
                  return (
                    <div key={step} className="flex items-start gap-2 text-[11px]">
                      {failedHere ? (
                        <XCircle size={12} className="mt-0.5 shrink-0 text-red-400" />
                      ) : done ? (
                        <CheckCircle2 size={12} className="mt-0.5 shrink-0 text-emerald-500" />
                      ) : isCurrent ? (
                        <Loader2 size={12} className="mt-0.5 shrink-0 animate-spin text-muted-foreground" />
                      ) : (
                        <span className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-muted-foreground/30" />
                      )}
                      <span
                        className={cn(
                          failedHere ? 'text-red-400' : done ? 'text-foreground/90' : 'text-muted-foreground',
                        )}
                      >
                        {CONNECT_STEP_LABELS[step]}
                        {failedHere && connectFlow.failure && (
                          <span className="mt-0.5 block text-red-400/90">{connectFlow.failure.message}</span>
                        )}
                      </span>
                    </div>
                  )
                })}
              </div>
            )}

            {/* Two surfaces, two facts — never one global verdict.
                `ai_configured` describes ONLY the REST surface
                (/api/ai/command needs a native provider with a real
                credential); the agent surface (/acp) is the LIVE session
                fact (`acpLive`), never just "a provider is pinned" — the
                same distinction the chip and the readout above now make.
                Collapsing "pinned" and "running" into one boolean here is
                exactly the bug this whole dialog was rewritten to remove.
                The agent-surface Claim only renders when nothing is
                saved: once `data.active` exists, the header card above
                already carries this exact fact ("agent running"/"agent
                not started") plus the pinned provider/mode — repeating it
                here would be the restatement Varun flagged. The one case
                that fact has nowhere else to live is `data.active ===
                null`, where no header card renders at all — that case
                keeps its own line, tri-state per card-chrome's Claim
                contract: amber "not asserted" (the boot fallback may or
                may not hold a credential — the backend hasn't claimed
                either), never a guessed tick or cross. */}
            <div className="flex flex-col gap-1 rounded-md border border-border/60 bg-background/40 px-3 py-2">
              {data.active === null && (
                <Claim status={null} detail="whatever the backend resolved at boot">
                  Agent surface (/acp) — no saved provider
                </Claim>
              )}
              <Claim
                status={data.ai_configured}
                detail={
                  data.ai_configured
                    ? 'native provider registered'
                    : 'needs an API key or OAuth credential — tool_use is not carried over the CLI transport'
                }
              >
                REST surface (/api/ai/command)
              </Claim>
            </div>

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
                const wireState = providerWireState(data, provider)
                return (
                  <Tooltip key={provider.id}>
                    <TooltipTrigger
                      render={
                        <button
                          type="button"
                          onClick={() => selectProvider(provider)}
                          aria-label={`${provider.display_name} (${STATE_LABELS[wireState]})`}
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
                          {/* Every mark carries this dot, always — not just
                              the active one. Colour is the ONLY thing
                              distinguishing the four states here; "wired but
                              not connected" (amber/blue) must never look like
                              "connected" (emerald) at a glance across the
                              row. What each colour MEANS used to be spelled
                              out in a standing legend below this row — that
                              read as explanation, not recognition, so it
                              moved into this tooltip (same text, on the dot
                              itself), reachable by hover AND by keyboard
                              focus (Base UI's Tooltip.Trigger opens on
                              `onFocus`, not just `onMouseOver`). */}
                          <span
                            className={cn(
                              'absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full ring-2 ring-background',
                              STATE_STYLES[wireState].dot,
                            )}
                            aria-hidden="true"
                          />
                        </button>
                      }
                    />
                    <TooltipContent>
                      {provider.display_name} — {STATE_LABELS[wireState]}
                    </TooltipContent>
                  </Tooltip>
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
                    const state = modeWireState(data, entry, selectedProvider)
                    const disabled = state === 'unavailable'
                    const status = modeStatus(data, entry, selectedProvider)
                    // Only present when `wiring.status === 'seam_only'` —
                    // the backend's own reason, verbatim. It used to render
                    // as a standing paragraph under every seam-only row
                    // (permanent shelf space for a mode most sessions never
                    // pick); it now lives in the row's own tooltip below,
                    // same text, reachable on demand instead of always on
                    // screen.
                    const seamReason = entry.wiring.status === 'seam_only' ? entry.wiring.reason : null
                    return (
                      <Tooltip key={entry.mode}>
                        <TooltipTrigger
                          render={
                            <button
                              type="button"
                              // `aria-disabled`, NOT the native `disabled`
                              // attribute: a natively disabled button can't
                              // receive focus or hover in most browsers,
                              // which would make this row's tooltip —
                              // carrying the ONLY explanation for why a
                              // seam-only mode isn't selectable — reachable
                              // by mouse-hover only, breaking Varun's "a row
                              // readable without a mouse" rule for exactly
                              // the row that most needs it. The click
                              // handler below still refuses the action.
                              aria-disabled={disabled}
                              onClick={() => {
                                if (disabled) return
                                setSelectedMode(entry.mode)
                                setTestResult(null)
                                setTestedFor(null)
                                // Unreachable today (every discovery-capable
                                // vendor is api_key-only, so switching modes
                                // within one vendor can't currently straddle
                                // a discovery result) — reset anyway for the
                                // same staleness discipline applied to every
                                // other per-key/per-mode piece of state here.
                                setDiscovery(null)
                                setSaveError(null)
                              }}
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
                                <span
                                  className={cn('h-1.5 w-1.5 shrink-0 rounded-full', STATE_STYLES[state].dot)}
                                  aria-hidden="true"
                                />
                                {MODE_LABELS[entry.mode]}
                                {entry.spawns_local_process && (
                                  <Terminal
                                    size={10}
                                    className="text-amber-400/90"
                                    aria-label="Spawns a local process on this machine"
                                  />
                                )}
                              </span>
                              <span className={cn('shrink-0', STATE_STYLES[state].text)}>{status.text}</span>
                            </button>
                          }
                        />
                        <TooltipContent>{seamReason ?? entry.reason}</TooltipContent>
                      </Tooltip>
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
                            setDiscovery(null)
                          }}
                          autoComplete="off"
                          className="h-7 text-[11px]"
                        />
                        {/* Client-side shape pre-flight — mirrors the
                            backend's `reject_implausible_key_shape`
                            (the actual enforcement point for
                            POST /api/ai/provider/models). Catches the
                            incident this closes — a 649-char multi-line
                            Vite error pasted in as a "key" — before any
                            network call, for either action below. */}
                        {apiKey && implausibleApiKeyReason(apiKey) && (
                          <p className="text-[10px] text-red-400">
                            API key {implausibleApiKeyReason(apiKey)}
                          </p>
                        )}
                        <div className="flex items-center gap-2">
                          <Button
                            variant="outline"
                            size="sm"
                            className="h-7 px-2 text-[11px]"
                            disabled={!apiKey || testing || !!implausibleApiKeyReason(apiKey)}
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
                        {/* Live model discovery — its own explicit action,
                            not fired on blur (which would spam the vendor
                            on every focus change). Separate from `Test`
                            above: for every non-anthropic api_key vendor
                            `Test` accepts the key without a network round
                            trip (see this component's own module doc), so
                            THIS is the one action that earns a genuine
                            "Verified" claim for those vendors. */}
                        <div className="flex items-center gap-2 border-t border-border/40 pt-2">
                          <Button
                            variant="outline"
                            size="sm"
                            className="h-7 px-2 text-[11px]"
                            disabled={
                              !apiKey ||
                              discovery?.phase === 'discovering' ||
                              !!implausibleApiKeyReason(apiKey)
                            }
                            onClick={() => void runDiscovery()}
                          >
                            {discovery?.phase === 'discovering'
                              ? 'Discovering…'
                              : 'Discover models'}
                          </Button>
                          {discovery?.phase === 'success' && discovery.apiKey === apiKey && (
                            <span className="flex items-center gap-1 text-[11px] text-emerald-400">
                              <CheckCircle2 size={12} />
                              Verified — {discovery.models.length}{' '}
                              {discovery.models.length === 1 ? 'model' : 'models'} at{' '}
                              {discovery.baseUrl}
                            </span>
                          )}
                          {discovery?.phase === 'error' && discovery.apiKey === apiKey && (
                            <span className="flex items-center gap-1 text-[11px] text-red-400">
                              <XCircle size={12} />
                              {discovery.message}
                            </span>
                          )}
                        </div>
                        <p className="text-[10px] text-muted-foreground">
                          Asks the vendor&apos;s own model-listing endpoint what it actually
                          serves — never a stored or guessed list.
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

                    {/* No `workload_identity` branch here: this whole panel
                        is gated on `selectedEntry.wiring.status === 'wired'`
                        above, and `workload_identity` is `SeamOnly` for
                        every provider that lists it (`allowlist.rs`) — so
                        that branch could never render. Its explanation (WIF
                        env vars are detected but the token exchange isn't
                        wired yet) lives on the mode row's own tooltip,
                        alongside the "not yet wired" status text, which is
                        the state that's actually reachable. */}

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
                      {discoveredModels ? (
                        // A real, vendor-returned list is in hand — a
                        // picker over it, never free text (an id typed by
                        // hand could silently diverge from what discovery
                        // just proved this key can serve). Never merged
                        // with `MODEL_PRESETS`: only ids the vendor itself
                        // named, plus "default".
                        <select
                          id="provider-model-select"
                          value={model}
                          onChange={(e) => applyModel(e.target.value)}
                          className="cad-focus h-7 rounded border border-border/60 bg-background/40 px-1.5 text-[11px] text-foreground/90 hover:bg-accent/30"
                        >
                          <option value="">default (provider&apos;s choice)</option>
                          {discoveredModels.map((m) => (
                            <option key={m.id} value={m.id}>
                              {m.id}
                              {m.context_limit ? ` (${m.context_limit.toLocaleString()} ctx)` : ''}
                            </option>
                          ))}
                        </select>
                      ) : (
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
                      )}
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
                      {!discoveredModels && modelCustom && (
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
                        {discoveredModels
                          ? discoveredModels.length > 0
                            ? "The vendor's own model list — not a suggestion."
                            : 'The vendor returned zero models for this key.'
                          : selectedMode === 'subscription_cli'
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
                        onClick={() => void runConnect({ save: true })}
                      >
                        {saving
                          ? 'Saving…'
                          : connectFlow?.status === 'running'
                            ? 'Connecting…'
                            : isConfigured
                              ? 'Saved'
                              : 'Save'}
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
