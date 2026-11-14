import { useCallback, useEffect, useState } from 'react'
import { useSketchBench } from '@/stores/sketch-bench-store'
import { HISTORY_CHANGED_EVENT } from '@/lib/history'
import { Check, PencilRuler, Trash2, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Box, ToolIcons } from '@/components/ui/icons'
import { useChapters } from '@/hooks/use-chapters'
import { useHistoryView } from '@/stores/history-view-store'
import { useFeatureLocationStore } from '@/stores/feature-location-store'
import { chapterIconKey, chapterTitle, type Chapter } from '@/lib/chapters-api'
import {
  applyEdit,
  deleteBody,
  deleteFeature,
  draftKey,
  featureFields,
  fetchEditableParameters,
  mouldBody,
  parseSize,
  type EditableParameter,
  type FeatureField,
} from '@/lib/feature-edit'

/** The chapter with this anchor, including a pattern's instances. */
function findChapter(chapters: Chapter[], anchor: number | null): Chapter | null {
  if (anchor === null) return null
  for (const c of chapters) {
    if (c.anchor === anchor) return c
    const inner = findChapter(c.instances, anchor)
    if (inner) return inner
  }
  return null
}

/** `drafts` without `key`'s typing. */
function without(drafts: Record<string, string>, key: string): Record<string, string> {
  const next = { ...drafts }
  delete next[key]
  return next
}

type Outcome = { key: string; kind: 'busy' } | { key: string; kind: 'refused'; reason: string } | null

/**
 * The selected feature's sizes, editable (Varun 2026-11-13: "why am I not
 * able to edit a feature as I select it?"). Each size is a recorded number
 * of the feature's steps; Enter applies it. The kernel tries the change on a
 * scratch copy first and refuses, in its own words, when a later feature
 * would break or the part would not be sound -- nothing changes then.
 */
/** Features made from a sketch: their sketch can be reopened and edited. */
const SKETCH_FEATURES = new Set(['extrude', 'cut', 'boss', 'revolve'])

export function FeatureEditPanel({
  branch = 'main',
  placement = 'column',
}: {
  branch?: string
  /** `callout`: on the part, at the arrow (when the feature was found on it);
   *  `column`: under the tree (when it was not -- e.g. a fillet). */
  placement?: 'column' | 'callout'
}) {
  const located = useFeatureLocationStore((s) => s.located)
  const selected = useHistoryView((s) => s.selectedChapter)
  const select = useHistoryView((s) => s.selectChapter)
  const { chapters } = useChapters(branch)
  const chapter = findChapter(chapters, selected)

  const [params, setParams] = useState<EditableParameter[]>([])
  // Why the sizes could not be read (the server unreachable or refusing).
  const [loadError, setLoadError] = useState<string | null>(null)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [outcome, setOutcome] = useState<Outcome>(null)
  const [reload, setReload] = useState(0)
  // Deleting the feature: busy, or the kernel's refusal (on this card).
  const [deleting, setDeleting] = useState<{ anchor: number; refused: string | null; busy: boolean } | null>(null)

  // Delete the selected feature (the card's 🗑, or the Delete key). The kernel
  // trials it and refuses -- in its own words -- when a later feature is
  // built on it; Undo brings a deleted feature back.
  const remove = useCallback(async () => {
    if (!chapter) return
    setDeleting({ anchor: chapter.anchor, refused: null, busy: true })
    const result = await deleteFeature(deleteBody(chapter))
    if (result.ok) {
      setDeleting(null)
      select(null)
      window.dispatchEvent(new CustomEvent(HISTORY_CHANGED_EVENT, { detail: { action: 'delete' } }))
    } else {
      setDeleting({ anchor: chapter.anchor, refused: result.reason, busy: false })
    }
  }, [chapter, select])

  // The Delete key deletes the selected feature -- not while a size is being
  // typed (the key then belongs to the field). One card owns the key: the
  // callout when the feature is on the part, else the column card.
  const sketching = useSketchBench((s) => s.sketchId !== null)
  const ownsKey = !sketching && chapter !== null && (placement === 'callout') === located.has(chapter.anchor)
  useEffect(() => {
    if (!ownsKey) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Delete') return
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
      e.preventDefault()
      void remove()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [ownsKey, remove])

  // The sizes of the history as it is now -- refetched when the selection
  // or the history changes (an edit appends a step, so `chapters` changes).
  useEffect(() => {
    if (selected === null) return
    let live = true
    fetchEditableParameters(branch)
      .then((p) => {
        if (!live) return
        setParams(p)
        setLoadError(null)
      })
      .catch((e: unknown) => {
        if (!live) return
        setParams([])
        setLoadError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      live = false
    }
  }, [branch, selected, chapters, reload])

  if (!chapter) return null
  const onPart = located.has(chapter.anchor)
  if (placement === 'column' ? onPart : !onPart) return null

  const fields = featureFields(chapter, params)
  const key = chapterIconKey(chapter.kind)
  const Icon = key ? ToolIcons[key] : Box
  const title = chapterTitle(chapter)

  const apply = async (field: FeatureField) => {
    const dk = draftKey(chapter.anchor, field.key)
    const text = drafts[dk]
    if (text === undefined) return
    const value = parseSize(text)
    if (value === null || value === field.value) {
      setDrafts((d) => without(d, dk))
      return
    }
    setOutcome({ key: dk, kind: 'busy' })
    const result = await applyEdit(mouldBody(field, value))
    if (result.ok) {
      setOutcome(null)
      setDrafts((d) => without(d, dk))
      setReload((n) => n + 1)
    } else {
      setOutcome({ key: dk, kind: 'refused', reason: result.reason })
    }
  }

  return (
    <section
      aria-label={`Edit ${title}`}
      className={cn(
        'overflow-hidden rounded-lg border bg-card text-xs',
        placement === 'callout' ? 'w-60 border-[var(--cad-selected)] shadow-lg' : 'border-border bg-card/95 shadow-sm',
      )}
    >
      <div className="flex items-center gap-2 px-2.5 pb-1.5 pt-2">
        <Icon size={17} />
        <span className="min-w-0 flex-1 truncate text-[12.5px] font-semibold">
          {title.charAt(0).toUpperCase() + title.slice(1)}
        </span>
        {SKETCH_FEATURES.has(chapter.kind) && (
          <button
            type="button"
            aria-label={`Edit the sketch of ${title}`}
            title="Edit sketch -- reopen it, change it, Apply"
            onClick={() => {
              void useSketchBench
                .getState()
                .editFeatureSketch(chapter.events, title)
                .then((r) => {
                  if (!r.ok) setDeleting({ anchor: chapter.anchor, refused: r.reason, busy: false })
                })
            }}
            className="cad-focus rounded p-0.5 text-muted-foreground hover:text-foreground"
          >
            <PencilRuler size={13} />
          </button>
        )}
        <button
          type="button"
          aria-label={`Delete ${title}`}
          title="Delete this feature (Delete key) -- Undo brings it back"
          disabled={deleting?.busy === true}
          onClick={() => void remove()}
          className="cad-focus rounded p-0.5 text-muted-foreground hover:text-destructive disabled:opacity-40"
        >
          <Trash2 size={13} />
        </button>
        <button type="button" aria-label="Close" onClick={() => select(null)} className="cad-focus rounded p-0.5 text-muted-foreground hover:text-foreground">
          <X size={13} />
        </button>
      </div>
      {deleting && deleting.anchor === chapter.anchor && (deleting.busy || deleting.refused) && (
        <p className={cn('px-2.5 pb-1.5 text-[11px]', deleting.refused ? 'text-red-700 dark:text-red-300' : 'text-muted-foreground')}>
          {deleting.busy ? 'Deleting…' : `Not done: ${deleting.refused}`}
        </p>
      )}
      {loadError ? (
        <p className="px-2.5 pb-2.5 text-[11px] text-red-700 dark:text-red-300">
          Could not read this feature's sizes ({loadError}). Is the server running?
        </p>
      ) : fields.length === 0 ? (
        <p className="px-2.5 pb-2.5 text-[11px] text-muted-foreground">
          This feature has no sizes to edit here.
        </p>
      ) : (
        <div className="flex flex-col gap-1.5 px-2.5 pb-2.5">
          {fields.map((f) => {
            const dk = draftKey(chapter.anchor, f.key)
            const text = drafts[dk] ?? String(f.value)
            const bad = drafts[dk] !== undefined && parseSize(text) === null
            const mine = outcome?.key === dk ? outcome : null
            return (
              <div key={f.key} className="flex flex-col gap-0.5">
                <label className="flex items-center gap-2">
                  <span className="w-20 shrink-0 text-muted-foreground">{f.label}</span>
                  <span
                    className={cn(
                      'flex h-6 min-w-0 flex-1 items-center rounded border bg-background/80 px-1.5',
                      bad ? 'border-red-500/60' : drafts[dk] !== undefined ? 'border-[#2F6FDB]' : 'border-border/70',
                    )}
                  >
                    <input
                      value={text}
                      inputMode="decimal"
                      disabled={mine?.kind === 'busy'}
                      onChange={(e) => {
                        setDrafts((d) => ({ ...d, [dk]: e.target.value }))
                        if (outcome?.key === dk) setOutcome(null)
                      }}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') void apply(f)
                        if (e.key === 'Escape') {
                          setDrafts((d) => without(d, dk))
                        }
                      }}
                      className="w-full min-w-0 bg-transparent font-mono outline-none"
                    />
                    <span className="text-muted-foreground">{f.unit}</span>
                  </span>
                  <span className="w-3 shrink-0" title={f.edited ? 'edited from its first value' : undefined}>
                    {f.edited && <span className="block h-1.5 w-1.5 rounded-full bg-[#2F6FDB]" />}
                  </span>
                </label>
                {mine?.kind === 'busy' && <span className="pl-[5.5rem] text-[11px] text-muted-foreground">Rebuilding…</span>}
                {mine?.kind === 'refused' && (
                  <span className="pl-[5.5rem] text-[11px] text-red-700 dark:text-red-300">Not applied: {mine.reason}</span>
                )}
              </div>
            )
          })}
          {placement === 'column' && (
            <p className="text-[11px] text-muted-foreground">
              Not marked on the part: this feature records no shape to find it by.
            </p>
          )}
          {chapter.instances.length > 0 && (
            <p className="text-[11px] text-muted-foreground">Applies to all {chapter.instances.length} instances.</p>
          )}
          <p className="flex items-center gap-1.5 border-t border-border/60 pt-1.5 text-[11px] text-muted-foreground">
            {chapter.sound ? (
              <>
                <Check size={11} className="text-emerald-600" /> Sound · Enter applies a size; Esc undoes the typing
              </>
            ) : (
              <span className="text-destructive">Not sound: {chapter.refusals[0]?.reason ?? 'see the history'}</span>
            )}
          </p>
        </div>
      )}
    </section>
  )
}
