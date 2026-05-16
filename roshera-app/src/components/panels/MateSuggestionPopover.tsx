/**
 * MateSuggestionPopover — pick-driven mate confirmation surface.
 *
 * Renders whenever `useAssemblyStore.pendingMate.ref1` is populated.
 * On the first pick the popover anchors at `ref1.screen` and arms
 * `Coincident` by default; clicking another chip simply re-arms the
 * mate type without committing. When the user picks a second face on
 * a different component (which lands as `pendingMate.ref2`), the
 * popover auto-commits with whatever chip is currently armed — no
 * separate confirmation click.
 *
 * Mate types offered:
 *
 *   - Coincident — primary suggestion for two planar faces. Locks the
 *     two surface planes flush against each other.
 *   - Parallel — same plane orientation without enforcing coplanarity.
 *   - Distance — Coincident plus an offset; the user types the gap.
 *
 * Concentric (axis-axis) is intentionally not offered: the per-
 * component mesh handler doesn't surface cylinder-axis metadata, so
 * we can't recover the axis from a face pick without a backend round-
 * trip. That suggestion will land alongside an axis-aware mesh
 * payload in a follow-up slice.
 *
 * Cancel paths:
 *   - Escape key
 *   - Empty-space click in the viewport (`onPointerMissed` in
 *     CADViewport calls `clearPendingMate` via the AssemblyWorkspace
 *     mount)
 *   - Switching active assembly (store-level reset)
 *
 * After a commit, the popover clears the pending mate and pipes the
 * post-solve snapshot through `setSnapshot` so the sidebar and gizmo
 * re-seed to the solver's resting pose.
 */

import { useEffect, useRef, useState } from 'react'
import { useAssemblyStore, type PendingPick } from '@/stores/assembly-store'
import { useSceneStore } from '@/stores/scene-store'
import {
  addMateFromPicks,
  type MateType,
  type PickRef,
} from '@/lib/assembly-api'

type Suggestion = 'Coincident' | 'Parallel' | 'Distance'

const SUGGESTIONS: readonly Suggestion[] = ['Coincident', 'Parallel', 'Distance']

export function MateSuggestionPopover() {
  const activeId = useAssemblyStore((s) => s.activeId)
  const active = useAssemblyStore((s) => s.active)
  const pendingMate = useAssemblyStore((s) => s.pendingMate)
  const clearPendingMate = useAssemblyStore((s) => s.clearPendingMate)
  const setSnapshot = useAssemblyStore((s) => s.setSnapshot)
  const setError = useAssemblyStore((s) => s.setError)
  const selectionMode = useSceneStore((s) => s.selectionMode)

  // Leaving face mode mid-flow invalidates the picks (they came from
  // a face-mode click path), so drop them.
  useEffect(() => {
    if (selectionMode !== 'face' && (pendingMate.ref1 || pendingMate.ref2)) {
      clearPendingMate()
    }
  }, [selectionMode, pendingMate.ref1, pendingMate.ref2, clearPendingMate])

  const [busy, setBusy] = useState<boolean>(false)
  const [armed, setArmed] = useState<Suggestion>('Coincident')
  const [distance, setDistance] = useState<string>('0.0')
  // Latch the in-flight commit so the auto-commit effect can't refire
  // (e.g. if the popover briefly re-renders before clearPendingMate
  // unmounts it).
  const committedRef = useRef(false)

  // Reset arm/distance/commit-latch when a fresh ref1 starts.
  // Without this, re-picking after Esc would inherit stale state.
  useEffect(() => {
    if (!pendingMate.ref1) {
      committedRef.current = false
      setArmed('Coincident')
      setDistance('0.0')
    }
  }, [pendingMate.ref1])

  // Esc clears the pending mate. Bound at this level (not globally)
  // so other parts of the app keep their Esc semantics.
  useEffect(() => {
    if (!pendingMate.ref1) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        clearPendingMate()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [pendingMate.ref1, clearPendingMate])

  const { ref1, ref2 } = pendingMate

  const toPickRef = (p: PendingPick): PickRef => ({
    componentId: p.componentId,
    origin: p.origin,
    normal: p.normal,
  })

  const buildMateType = (suggestion: Suggestion): MateType | null => {
    if (suggestion === 'Distance') {
      const d = Number(distance)
      if (!Number.isFinite(d)) {
        setError('Distance must be a finite number')
        return null
      }
      return { Distance: d }
    }
    return suggestion
  }

  const commit = async (
    suggestion: Suggestion,
    r1: PendingPick,
    r2: PendingPick,
  ) => {
    if (!activeId || !active) return
    const mateType = buildMateType(suggestion)
    if (mateType === null) {
      committedRef.current = false
      clearPendingMate()
      return
    }
    setBusy(true)
    try {
      const snap = await addMateFromPicks(
        activeId,
        active,
        toPickRef(r1),
        toPickRef(r2),
        mateType,
      )
      setSnapshot(snap)
      clearPendingMate()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      committedRef.current = false
    } finally {
      setBusy(false)
    }
  }

  // Second pick lands → auto-commit with whatever chip is armed.
  useEffect(() => {
    if (!ref1 || !ref2) return
    if (committedRef.current) return
    committedRef.current = true
    void commit(armed, ref1, ref2)
    // commit/armed/distance are captured by closure at fire time —
    // the latch prevents the effect from re-firing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ref1, ref2])

  if (!activeId || !active || !ref1) {
    return null
  }
  const anchor = ref2?.screen ?? ref1.screen

  // Position the popover next to the click point, but clamp inside
  // the viewport so the panel never spills off-screen on edge picks.
  const PANEL_W = 220
  const PANEL_H = 168
  const margin = 8
  const left = Math.min(
    Math.max(margin, anchor.x + 12),
    (typeof window !== 'undefined' ? window.innerWidth : 1280) - PANEL_W - margin,
  )
  const top = Math.min(
    Math.max(margin, anchor.y + 12),
    (typeof window !== 'undefined' ? window.innerHeight : 720) - PANEL_H - margin,
  )

  return (
    <div
      className="fixed z-50 cad-panel px-2.5 py-2 text-[11px] uppercase tracking-wider shadow-lg"
      style={{ left, top, width: PANEL_W }}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-foreground font-semibold">New mate</span>
        <button
          type="button"
          onClick={clearPendingMate}
          className="px-1.5 py-0.5 border border-border/60 hover:border-border text-muted-foreground hover:text-foreground transition-colors"
          title="Cancel (Esc)"
        >
          ✕
        </button>
      </div>
      <div className="flex flex-col gap-1">
        {SUGGESTIONS.map((s) =>
          s === 'Distance' ? (
            <div key={s} className="flex items-center gap-1">
              <input
                type="number"
                value={distance}
                onChange={(e) => setDistance(e.target.value)}
                step="0.1"
                className="w-16 px-1.5 py-1 bg-background border border-border/60 text-foreground tabular-nums font-mono text-[11px]"
                title="Distance in mm"
                disabled={busy}
              />
              <SuggestionButton
                label="Distance"
                armed={armed === 'Distance'}
                busy={busy && armed === 'Distance'}
                disabled={busy}
                onClick={() => setArmed('Distance')}
                stretch
              />
            </div>
          ) : (
            <SuggestionButton
              key={s}
              label={s}
              armed={armed === s}
              busy={busy && armed === s}
              disabled={busy}
              onClick={() => setArmed(s)}
            />
          ),
        )}
      </div>
      <div className="mt-2 normal-case tracking-normal text-[10px] text-muted-foreground">
        {busy
          ? 'Solving…'
          : ref2
          ? 'Mate ready — committing.'
          : 'Click a face on another component to apply the armed mate.'}
      </div>
    </div>
  )
}

function SuggestionButton({
  label,
  armed,
  busy,
  disabled,
  onClick,
  stretch,
}: {
  label: string
  armed: boolean
  busy: boolean
  disabled: boolean
  onClick: () => void
  stretch?: boolean
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={[
        'px-2 py-1 border',
        armed
          ? 'border-primary bg-primary/10 text-foreground'
          : 'border-border/60 hover:border-border text-muted-foreground hover:text-foreground',
        'disabled:opacity-50 disabled:cursor-not-allowed',
        'transition-colors text-left',
        stretch ? 'flex-1' : 'w-full',
      ].join(' ')}
    >
      {busy ? '…' : label}
    </button>
  )
}
