import { cn } from '@/lib/utils'
import { useSceneStore, type SelectionMode } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useCursorStore } from '@/stores/cursor-store'
import { useChapters } from '@/hooks/use-chapters'
import { formatPoint, soundLabel, underCursorLabel } from '@/lib/status-bar'

const PICKS: { mode: SelectionMode; label: string; key: string }[] = [
  { mode: 'object', label: 'Part', key: '1' },
  { mode: 'face', label: 'Face', key: '2' },
  { mode: 'edge', label: 'Edge', key: '3' },
  { mode: 'vertex', label: 'Vertex', key: '4' },
]

const TONE = {
  positive: 'text-emerald-700 dark:text-emerald-400',
  negative: 'text-red-700 dark:text-red-400',
  muted: 'text-muted-foreground',
} as const

function Divider() {
  return <span aria-hidden className="mx-2 h-3 w-px bg-border" />
}

/**
 * The status bar (the approved mockup): what a click picks, snap, where the
 * cursor is on the part and what it is over -- then the part count, the
 * history's verdict and the connection. The numbers live here, not in panels.
 */
export function StatusBar() {
  const objectCount = useSceneStore((s) => s.objects.size)
  const selectedCount = useSceneStore((s) => s.selectedIds.size)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)
  const snapMode = useSceneStore((s) => s.snapMode)
  const setSnapMode = useSceneStore((s) => s.setSnapMode)
  const snapValue = useSceneStore((s) => s.snapValue)
  const lastPing = useWSStore((s) => s.lastPingMs)
  const mode = useDocModeStore((s) => s.mode)
  const hit = useCursorStore((s) => s.hit)
  const hitName = useSceneStore((s) => (hit ? (s.objects.get(hit.objectId)?.name ?? 'part') : null))
  const { chapters, loading, error, isSound } = useChapters('main')

  const verdict = soundLabel({ loaded: !loading && !error, sound: isSound, empty: chapters.length === 0 })
  const ping = lastPing !== null && (
    <>
      <Divider />
      <span className="tabular-nums">{lastPing} ms</span>
    </>
  )

  // On a drawing the modelling state means nothing: only the connection.
  if (mode === 'drawing') {
    return (
      <div className="flex h-6 items-center border-t border-border bg-background px-3 text-[11px] text-muted-foreground">
        <span className="font-medium uppercase tracking-wider">Drawing</span>
        <div className="flex-1" />
        {ping}
      </div>
    )
  }

  return (
    <div className="flex h-6 items-center border-t border-border bg-background px-2 text-[11px] text-muted-foreground">
      <span className="mr-1.5">Pick</span>
      <div role="radiogroup" aria-label="What a click picks" className="flex items-center gap-px">
        {PICKS.map((p) => (
          <button
            key={p.mode}
            type="button"
            role="radio"
            aria-checked={selectionMode === p.mode}
            title={`${p.label} · ${p.key}`}
            onClick={() => setSelectionMode(p.mode)}
            className={cn(
              'cad-focus h-[18px] rounded px-1.5 leading-none transition-colors',
              selectionMode === p.mode
                ? 'bg-[var(--cad-selected-wash)] font-medium text-foreground'
                : 'hover:bg-accent hover:text-foreground',
            )}
          >
            {p.label}
          </button>
        ))}
      </div>

      <Divider />
      <button
        type="button"
        aria-pressed={snapMode !== 'none'}
        onClick={() => setSnapMode(snapMode === 'none' ? 'grid' : 'none')}
        title="Snap moves to the grid"
        className="cad-focus h-[18px] rounded px-1.5 leading-none hover:bg-accent hover:text-foreground"
      >
        Snap <span className="text-foreground">{snapMode !== 'none' ? `${snapValue} mm` : 'off'}</span>
      </button>

      <Divider />
      <span className="w-[13.5rem] font-mono tabular-nums text-foreground/80" title="The cursor's point on the part">
        {formatPoint(hit?.point ?? null)}
      </span>
      <Divider />
      <span className="min-w-0 truncate">
        Under cursor:{' '}
        <span className="text-foreground">
          {underCursorLabel(hit && hitName ? { part: hitName, faceId: hit.faceId } : null)}
        </span>
      </span>

      <div className="flex-1" />

      <span className="tabular-nums">
        {objectCount} part{objectCount !== 1 ? 's' : ''}
        {selectedCount > 0 && ` · ${selectedCount} selected`}
      </span>
      <Divider />
      <span className={cn('font-medium', TONE[verdict.tone])} title="Every step re-measured by the kernel">
        {verdict.text}
      </span>
      {ping}
    </div>
  )
}
