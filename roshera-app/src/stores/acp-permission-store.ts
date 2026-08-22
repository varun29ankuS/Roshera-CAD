import { create } from 'zustand'
import type { JsonRpcId } from '@/lib/acp-client'

/**
 * Permission requests the agent has made and the human has not yet answered.
 *
 * ## Why this store exists
 *
 * `session/request_permission` has always arrived. The client answered it
 * itself:
 *
 * ```ts
 * const chosen = params?.options?.find((o) => o.kind === 'allow_once') ?? params?.options?.[0]
 * await this.respondToServer(req.id, { outcome: { outcome: 'selected', optionId: chosen?.optionId } })
 * ```
 *
 * — picking `allow_once` when offered and otherwise whatever happened to be
 * first. The human never learned the agent had asked, and the options were
 * discarded unread.
 *
 * In a product whose thesis is that a person orchestrates and the kernel
 * cannot lie, the single moment the agent asks for judgement was the one
 * moment judgement was skipped. The fallback to `options[0]` is worse than the
 * `allow_once` preference: it would auto-select a destructive option purely
 * because the agent listed it first.
 *
 * So the request is held here, unanswered, until a human answers it. There is
 * deliberately NO client-side timeout: a stall is made visible rather than
 * resolved fictionally. The two honest endings are that the person decides, or
 * that the agent gives up and the wire tells us.
 */

/** A permission option exactly as the wire carried it. */
export interface PermissionOption {
  optionId: string
  /** ACP's option kind — `allow_once`, `allow_always`, `reject_once`, … */
  kind?: string
  /** Present only if the agent sent one; we never synthesise a label. */
  name?: string
}

export interface PendingPermission {
  /** JSON-RPC id the answer must carry back. */
  requestId: JsonRpcId
  options: PermissionOption[]
  /** Binds the card to a tool-call row when the agent sent the id. */
  toolCallId: string | null
  /** The agent's own words for what it is asking, when sent. */
  title: string | null
  description: string | null
  /** When WE received it. Ours to measure, so ours to display — unlike a
   *  tool-call duration, which would be a number this client invented. */
  askedAt: number
}

/** How a pending request ended, for the transcript record. */
export type PermissionOutcome =
  | { kind: 'answered'; optionId: string; label: string; allowed: boolean; waitedMs: number }
  | { kind: 'expired'; waitedMs: number }

interface PermissionState {
  pending: PendingPermission[]
  /** Set by the client so the card can answer without importing it. */
  respond: ((requestId: JsonRpcId, optionId: string) => void) | null

  open: (req: PendingPermission) => void
  /** Remove one request — after answering, or when the agent stops waiting. */
  close: (requestId: JsonRpcId) => void
  clear: () => void
  setResponder: (fn: ((requestId: JsonRpcId, optionId: string) => void) | null) => void
}

export const useAcpPermissionStore = create<PermissionState>((set) => ({
  pending: [],
  respond: null,

  open: (req) =>
    set((s) =>
      // A duplicate id would double-answer the same JSON-RPC request.
      s.pending.some((p) => p.requestId === req.requestId)
        ? s
        : { pending: [...s.pending, req] },
    ),

  close: (requestId) =>
    set((s) => ({ pending: s.pending.filter((p) => p.requestId !== requestId) })),

  // A dead session's questions are not answerable — the request id belongs to
  // a connection that no longer exists, so holding them would offer the human
  // a button that cannot post.
  clear: () => set({ pending: [] }),

  setResponder: (fn) => set({ respond: fn }),
}))

/**
 * The verb a button should show for an option kind.
 *
 * Kinds are rendered as verbs because that is all the wire gives us: today
 * `options[]` carries `{ optionId, kind }` and no `name`. When the agent does
 * send a name, the name wins — it is the agent's own words, and this map is
 * only a fallback for their absence.
 *
 * An unrecognised kind prints the kind itself rather than a guess. A button
 * labelled "Allow" that actually sends something else is the exact failure
 * this store was written to end.
 */
export function permissionVerb(option: PermissionOption): string {
  if (option.name && option.name.trim()) return option.name.trim()
  switch (option.kind) {
    case 'allow_once':
      return 'Allow once'
    case 'allow_always':
      return 'Allow always'
    case 'reject_once':
      return 'Deny'
    case 'reject_always':
      return 'Deny always'
    default:
      return option.kind ?? option.optionId
  }
}

/** Does choosing this option let the agent proceed? Used for hue only. */
export function permissionAllows(option: PermissionOption): boolean {
  return option.kind ? option.kind.startsWith('allow') : false
}
