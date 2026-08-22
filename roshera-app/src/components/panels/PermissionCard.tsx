import { useEffect, useState } from 'react'
import { cn } from '@/lib/utils'
import {
  permissionAllows,
  permissionVerb,
  useAcpPermissionStore,
  type PendingPermission,
} from '@/stores/acp-permission-store'

/** `1m12s`, or `42s` under a minute. */
function waited(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}s`
}

/**
 * The agent has asked for permission and is blocked until someone answers.
 *
 * This is the only clock in the transcript, and it is legal precisely because
 * it starts in THIS process: we received the request, we are holding it, so
 * "you have been sitting on this for 42 seconds" is a fact we own. A tool
 * call's duration would not be — the wire carries no start time — which is why
 * no other row here shows elapsed time.
 *
 * There is no timeout and no default. A request that nobody answers stays
 * open: a stall the human can see is better than one this client resolved on
 * their behalf and never mentioned.
 */
export function PermissionCard({ request }: { request: PendingPermission }) {
  const respond = useAcpPermissionStore((s) => s.respond)
  const close = useAcpPermissionStore((s) => s.close)
  const [now, setNow] = useState(() => Date.now())
  const [answering, setAnswering] = useState<string | null>(null)

  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [])

  const choose = (optionId: string) => {
    if (answering) return
    setAnswering(optionId)
    respond?.(request.requestId, optionId)
    close(request.requestId)
  }

  return (
    <div
      role="group"
      aria-label="The agent is asking for permission"
      className="my-1 w-full max-w-[72ch] rounded border border-caution-border bg-caution-wash px-3 py-2"
    >
      <div className="flex items-baseline gap-2">
        <span className="shrink-0 text-[11px] font-semibold uppercase tracking-wider text-caution">
          ▲ Permission
        </span>
        <span className="min-w-0 flex-1 font-mono text-[11px] tabular-nums text-muted-foreground">
          waiting {waited(now - request.askedAt)}
        </span>
      </div>

      {/* The agent's own words, when it sent any. Nothing is synthesised: a
          request with no title says so, rather than being given a sentence
          this component wrote about a decision it does not understand. */}
      {request.title ? (
        <p className="mt-1 text-sm leading-snug text-foreground">{request.title}</p>
      ) : (
        <p className="mt-1 text-sm leading-snug text-muted-foreground">
          The agent asked to proceed but sent no description of what it wants to do.
        </p>
      )}
      {request.description && (
        <p className="mt-0.5 text-[11px] leading-snug text-muted-foreground">
          {request.description}
        </p>
      )}
      {request.toolCallId && (
        <p className="mt-0.5 font-mono text-[11px] text-muted-foreground/70">
          {request.toolCallId}
        </p>
      )}

      {/* Options in WIRE ORDER, every one the agent offered and nothing more.
          No option is preselected and no button is styled as the obvious
          choice: a default biases a judge, and this card exists because a
          default was being applied silently. */}
      <div className="mt-2 flex flex-col gap-1">
        {request.options.map((opt, i) => {
          const allows = permissionAllows(opt)
          return (
            <button
              key={opt.optionId}
              type="button"
              disabled={answering !== null}
              onClick={() => choose(opt.optionId)}
              className={cn(
                'cad-focus flex w-full items-center gap-2 rounded border px-2 py-1 text-left text-[11px] transition-colors disabled:opacity-50',
                allows
                  ? 'border-positive-border text-positive hover:bg-positive-wash'
                  : 'border-negative-border text-destructive hover:bg-negative-wash',
              )}
            >
              <span className="shrink-0 font-mono text-muted-foreground">{i + 1}</span>
              <span className="min-w-0 flex-1 font-medium">{permissionVerb(opt)}</span>
              {opt.kind?.endsWith('always') && (
                <span className="shrink-0 text-[11px] uppercase tracking-wider text-caution">
                  changes future turns
                </span>
              )}
            </button>
          )
        })}
      </div>
    </div>
  )
}

/**
 * A sticky strip above the composer, so a decision cannot be missed by
 * someone whose eyes are on the model rather than the transcript.
 *
 * It never offers a verdict of its own — clicking scrolls to the card. A
 * bulk-approve control is deliberately absent: approving several unread asks
 * with one button is the auto-answer this whole change removed, wearing a
 * human's finger.
 */
export function PermissionStrip({ onFocus }: { onFocus?: () => void }) {
  const pending = useAcpPermissionStore((s) => s.pending)
  if (pending.length === 0) return null
  return (
    <button
      type="button"
      onClick={onFocus}
      className="cad-focus flex w-full items-center gap-2 border-t border-caution-border bg-caution-wash px-3 py-1.5 text-left text-[11px] text-caution"
    >
      <span aria-hidden>▲</span>
      <span className="min-w-0 flex-1 font-medium">
        {pending.length === 1
          ? 'The agent is waiting on your decision'
          : `${pending.length} decisions waiting`}
      </span>
      <span className="shrink-0 text-muted-foreground">show</span>
    </button>
  )
}
