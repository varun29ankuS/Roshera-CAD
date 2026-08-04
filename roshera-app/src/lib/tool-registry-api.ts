/**
 * REST client for the developer Semantics panel
 * (`components/dev/SemanticsPanel.tsx`).
 *
 * Four read-only sources, each mirrored from its verified backend shape:
 *
 * `GET /api/agent/tool-registry` — the kernel-served agent tool registry
 *   (`api-server/src/agent_registry.rs::build_registry`). The single
 *   server-side source of truth for the agent-facing operation inventory.
 *   The registry carries ONLY the backend's classification (`bench` is a
 *   column of its compiled tool table); the MCP's own `BENCH_OF`
 *   (`roshera-mcp/src/registry.ts`) is compiled into the MCP process and
 *   is NOT served over any HTTP endpoint — so a client cannot compare the
 *   two classifications from this response, and must not pretend to.
 *
 * `GET /api/timeline/history/{branch}` — recorded kernel events
 *   (`EventSummary`, typed in `lib/timeline-events.ts`).
 *
 * `GET /api/timeline/checkpoints` — declared intents over event ranges
 *   (`CheckpointSummary`, same module).
 *
 * `GET /api/evidence-pack?branch=…` — recorded operations WITH their
 *   certificates AS RECORDED (`EvidenceOperation` in
 *   `api-server/src/handlers/timeline.rs`): `certificate` is read verbatim
 *   from event metadata, `null` + `certificate_absent_reason` when the
 *   event carries none — never a fabricated green. This route sits behind
 *   the global auth layer (it is not on the public allowlist), so a 401
 *   here is a real, reportable state — the panel renders it as such
 *   instead of showing an invented count.
 */

import type { CheckpointSummary, EventSummary } from '@/lib/timeline-events'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

// ─── GET /api/agent/tool-registry ───────────────────────────────────

/** One row of the served tool table (agent_registry.rs::build_registry). */
export interface RegistryTool {
  name: string
  /** Backend bench classification: core|sketch|assembly|drawing|analysis|labels|timeline. */
  bench: string
  purpose: string
  /** JSON-Schema transcription of the tool's wire contract. */
  input_schema: unknown
  /** ceil(len/4) over compact {name,purpose,input_schema} — a budgeting proxy, not a tokenizer. */
  token_estimate: number
  stability: 'stable' | 'experimental' | (string & {})
  /** 'kernel' = purpose served from geometry-engine's op registry; 'curated' = transcribed zod. */
  source: 'kernel' | 'curated' | (string & {})
}

export interface ToolRegistry {
  /** fnv1a-64 over the canonicalized tools array — the MCP-side drift sentinel. */
  registry_hash: string
  generated_at: string
  hash_algorithm: string
  tool_count: number
  /** Served per-bench counts over the same array. */
  bench_counts: Record<string, number>
  tools: RegistryTool[]
}

export async function fetchToolRegistry(): Promise<ToolRegistry> {
  const resp = await fetch(`${API_BASE}/agent/tool-registry`, {
    headers: { Accept: 'application/json' },
  })
  if (!resp.ok) {
    throw new Error(`GET /api/agent/tool-registry → HTTP ${resp.status}`)
  }
  const data = (await resp.json()) as ToolRegistry
  if (!Array.isArray(data.tools) || typeof data.registry_hash !== 'string') {
    throw new Error(
      'GET /api/agent/tool-registry returned an unrecognised shape (no tools[] / registry_hash)',
    )
  }
  return data
}

// ─── GET /api/timeline/history/{branch} ─────────────────────────────

/**
 * Recorded events for `branch`. Mirrors the Timeline strip's tolerance for
 * both a bare array and an `{events: […]}` wrapper. An unrecognisable
 * payload throws — the caller reports failure, it never substitutes [].
 */
export async function fetchTimelineHistory(branch = 'main'): Promise<EventSummary[]> {
  const resp = await fetch(
    `${API_BASE}/timeline/history/${encodeURIComponent(branch)}`,
    { headers: { Accept: 'application/json' } },
  )
  if (!resp.ok) {
    throw new Error(`GET /api/timeline/history/${branch} → HTTP ${resp.status}`)
  }
  const data: unknown = await resp.json()
  if (Array.isArray(data)) return data as EventSummary[]
  if (data && typeof data === 'object' && Array.isArray((data as { events?: unknown }).events)) {
    return (data as { events: EventSummary[] }).events
  }
  throw new Error(`GET /api/timeline/history/${branch} returned an unrecognised shape`)
}

// ─── GET /api/timeline/checkpoints ──────────────────────────────────

export async function fetchCheckpoints(): Promise<CheckpointSummary[]> {
  const resp = await fetch(`${API_BASE}/timeline/checkpoints`, {
    headers: { Accept: 'application/json' },
  })
  if (!resp.ok) {
    throw new Error(`GET /api/timeline/checkpoints → HTTP ${resp.status}`)
  }
  const data: unknown = await resp.json()
  if (!Array.isArray(data)) {
    throw new Error('GET /api/timeline/checkpoints returned an unrecognised shape')
  }
  return data as CheckpointSummary[]
}

// ─── GET /api/evidence-pack ─────────────────────────────────────────

/** One recorded operation's evidence row (handlers/timeline.rs::EvidenceOperation). */
export interface EvidenceOperation {
  sequence: number
  event_id: string
  op_kind: string
  params: unknown
  timestamp: string
  author: string
  author_kind: string
  /** The certificate AS RECORDED on this event; `null` when it carries none. */
  certificate: unknown | null
  /** Present exactly when `certificate` is null — why, in the backend's words. */
  certificate_absent_reason?: string
}

export interface EvidencePack {
  manifest: {
    generated_at: string
    kernel_version: string
    operation_count: number
  }
  operations: EvidenceOperation[]
}

/** Thrown for a non-2xx evidence-pack response so the panel can name the status. */
export class EvidencePackHttpError extends Error {
  readonly status: number
  constructor(status: number) {
    super(`GET /api/evidence-pack → HTTP ${status}`)
    this.status = status
  }
}

export async function fetchEvidencePack(branch = 'main'): Promise<EvidencePack> {
  const resp = await fetch(
    `${API_BASE}/evidence-pack?branch=${encodeURIComponent(branch)}`,
    { headers: { Accept: 'application/json' } },
  )
  if (!resp.ok) {
    throw new EvidencePackHttpError(resp.status)
  }
  const data = (await resp.json()) as EvidencePack
  if (!Array.isArray(data.operations)) {
    throw new Error('GET /api/evidence-pack returned an unrecognised shape (no operations[])')
  }
  return data
}
