import { useMemo, useState } from 'react'
import { cn } from '@/lib/utils'
import { buildLanes, type Lane, type LaneTick, type LineageNode } from '@/lib/lineage-lanes'

/**
 * The document's history drawn as the lives of its solids.
 *
 * Its sibling view draws the same data as 150 operation cards, which answers
 * "what calls happened". This answers "what became of the part", which is the
 * question actually being asked of a CAD history. A row is one solid's life; a
 * mark on it is an operation that touched it; a boolean ends the operand rows
 * and carries one of them on.
 *
 * Nothing here is a card. At rest the whole document is marks on rows, because
 * 105 labelled boxes cannot be legible at any zoom and the 11px floor makes
 * that concrete rather than a matter of taste. Labels are earned by focus: a
 * row names itself, and clicking one opens its operations.
 */

/** Row geometry. Tight enough that a hundred rows are one picture. */
const ROW_H = 14
const LEFT_GUTTER = 132
const RIGHT_PAD = 16

/** Marks. Each one states a fact the wire carried; none is decorative. */
const MARK = {
  birth: '●',
  /** The lane took a new id here — every boolean does this to its result. */
  rebirth: '◆',
  merged: '✕',
  deleted: '■',
  live: '▶',
  dangling: '?',
  shadowed: '⚠',
} as const

function endMark(lane: Lane): { glyph: string; className: string; title: string } {
  switch (lane.end.kind) {
    case 'merged':
      return {
        glyph: MARK.merged,
        className: 'fill-muted-foreground',
        title: `consumed by a boolean at #${lane.end.atSequence} — its result carries on`,
      }
    case 'deleted':
      return {
        glyph: MARK.deleted,
        className: 'fill-muted-foreground/60',
        title: `deleted at #${lane.end.atSequence}`,
      }
    case 'live':
      return { glyph: MARK.live, className: 'fill-positive', title: 'still live' }
    case 'dangling':
      return {
        glyph: MARK.dangling,
        className: 'fill-caution',
        title:
          `consumed at #${lane.end.atSequence} with no successor recorded. ` +
          `Where this solid went is not in the history — the view reports the gap ` +
          `rather than drawing the lane as alive.`,
      }
    case 'shadowed':
      return {
        glyph: MARK.shadowed,
        className: 'fill-destructive',
        title:
          `its id was re-used by a new solid at #${lane.end.atSequence} while this one ` +
          `was still live, so its later history is unknowable. Broken here rather than ` +
          `stitched onto a different solid wearing the same name.`,
      }
  }
}

/** A lane's label: a declared name if one exists, otherwise its id. Never prose. */
function laneLabel(lane: Lane): string {
  return lane.name ?? lane.currentEntity
}

/**
 * Sort so the document's spine is at the top and its scaffolding below.
 *
 * Explicitly NOT chronological: birth order puts the first cutter above the
 * part it cuts. Length is the honest proxy for "this is a thing that had a
 * life", and living lanes lead because they are what the document still holds.
 */
function rank(a: Lane, b: Lane): number {
  const alive = (l: Lane) => (l.end.kind === 'live' ? 0 : 1)
  if (alive(a) !== alive(b)) return alive(a) - alive(b)
  if (a.ticks.length !== b.ticks.length) return b.ticks.length - a.ticks.length
  return a.bornAtSequence - b.bornAtSequence
}

export function LineageLanes({
  nodes,
  checkpoints,
}: {
  nodes: LineageNode[]
  checkpoints: Array<{ name: string; sequence_number?: number }>
}) {
  const [focused, setFocused] = useState<number | null>(null)
  const model = useMemo(() => buildLanes(nodes), [nodes])

  const seqs = nodes.map((n) => n.sequence_number)
  const minSeq = seqs.length ? Math.min(...seqs) : 0
  const maxSeq = seqs.length ? Math.max(...seqs) : 1
  const span = Math.max(1, maxSeq - minSeq)

  const ordered = useMemo(() => [...model.lanes].sort(rank), [model.lanes])
  const width = 1000
  const plotW = width - LEFT_GUTTER - RIGHT_PAD
  const x = (seq: number) => LEFT_GUTTER + ((seq - minSeq) / span) * plotW

  const eras = useMemo(() => {
    const marks = checkpoints
      .filter((c) => typeof c.sequence_number === 'number')
      .sort((a, b) => (a.sequence_number ?? 0) - (b.sequence_number ?? 0))
    return marks.map((c, i) => ({
      name: c.name,
      from: c.sequence_number ?? minSeq,
      to: marks[i + 1]?.sequence_number ?? maxSeq,
    }))
  }, [checkpoints, minSeq, maxSeq])

  const height = ordered.length * ROW_H + 8
  const counts = ordered.reduce<Record<string, number>>((acc, l) => {
    acc[l.end.kind] = (acc[l.end.kind] ?? 0) + 1
    return acc
  }, {})

  if (ordered.length === 0) {
    return (
      <div className="p-6 font-mono text-[11px] text-muted-foreground">
        No entity lineage recorded on this branch — every operation ran without naming what it
        consumed or produced, so there are no lives to draw.
      </div>
    )
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* The census IS the summary. Five living parts on a document of 150
          operations is the fact a reader wants first, and it is a count of
          rows rather than a claim about them. */}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border/60 px-3 py-1.5 font-mono text-[11px] text-muted-foreground">
        <span className="text-foreground">{ordered.length} solids</span>
        {counts.live ? <span className="text-positive">{counts.live} live</span> : null}
        {counts.merged ? <span>{counts.merged} consumed</span> : null}
        {counts.deleted ? <span>{counts.deleted} deleted</span> : null}
        {counts.dangling ? (
          <span
            className="text-caution"
            title="Consumed with no successor recorded — the history does not say where these went."
          >
            {counts.dangling} unaccounted
          </span>
        ) : null}
        {counts.shadowed ? (
          <span className="text-destructive" title="Their ids were re-used while they were live.">
            {counts.shadowed} id-collision
          </span>
        ) : null}
        {model.unattached.length ? (
          <span title="Operations that recorded no entity references at all. They touch no lane, by design.">
            {model.unattached.length} unattached ops
          </span>
        ) : null}
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        <svg
          width={width}
          height={height}
          viewBox={`0 0 ${width} ${height}`}
          className="block"
          role="img"
          aria-label={`${ordered.length} solid lifetimes across ${nodes.length} operations`}
        >
          {/* ERAS — a checkpoint is a stretch of time, so it is drawn once as a
              band rather than stamped onto every operation inside it. That
              repetition is why thirty cards in the sibling view all read the
              same sentence. */}
          {eras.map((era, i) => (
            <g key={`${era.name}-${era.from}`}>
              <rect
                x={x(era.from)}
                y={0}
                width={Math.max(1, x(era.to) - x(era.from))}
                height={height}
                className={i % 2 === 0 ? 'fill-muted/25' : 'fill-transparent'}
              />
              <title>{era.name}</title>
            </g>
          ))}

          {ordered.map((lane, row) => {
            const y = row * ROW_H + ROW_H / 2
            const first = lane.ticks[0]
            const last = lane.ticks[lane.ticks.length - 1]
            if (!first || !last) return null
            const x0 = x(first.node.sequence_number)
            const x1 = x(last.node.sequence_number)
            const isFocused = focused === lane.index
            const end = endMark(lane)

            return (
              <g
                key={lane.index}
                className="cursor-pointer"
                onClick={() => setFocused(isFocused ? null : lane.index)}
                opacity={focused === null || isFocused ? 1 : 0.25}
              >
                <rect x={0} y={row * ROW_H} width={width} height={ROW_H} className="fill-transparent" />

                {/* The label. A declared name when `part_rename` recorded one,
                    otherwise the id — an unnamed solid stays `solid:15`,
                    because it was not called anything. */}
                <text
                  x={LEFT_GUTTER - 8}
                  y={y + 3.5}
                  textAnchor="end"
                  className={cn(
                    'font-mono text-[11px]',
                    lane.name ? 'fill-foreground' : 'fill-muted-foreground',
                  )}
                >
                  {laneLabel(lane)}
                </text>

                {/* A lane whose producer is outside the served window begins
                    mid-story, so it enters from the edge as a dashed lead-in
                    rather than pretending to be born here. */}
                {lane.enteredFromBeforeWindow && (
                  <line
                    x1={LEFT_GUTTER - 4}
                    y1={y}
                    x2={x0}
                    y2={y}
                    strokeDasharray="2 2"
                    className="stroke-muted-foreground/40"
                    strokeWidth={1}
                  />
                )}

                <line
                  x1={x0}
                  y1={y}
                  x2={Math.max(x1, x0 + 2)}
                  y2={y}
                  className={cn(
                    lane.end.kind === 'live' ? 'stroke-positive/70' : 'stroke-muted-foreground/45',
                  )}
                  strokeWidth={isFocused ? 2 : 1}
                />

                {lane.ticks.map((tick: LaneTick, i) => (
                  <circle
                    key={`${tick.node.id}-${i}`}
                    cx={x(tick.node.sequence_number)}
                    cy={y}
                    r={tick.rebirth ? 2.4 : 1.6}
                    className={
                      tick.rebirth ? 'fill-primary' : 'fill-muted-foreground/70'
                    }
                  >
                    <title>
                      {`#${tick.node.sequence_number} ${tick.node.operation_type} · ${tick.entity} · ${tick.node.author}`}
                    </title>
                  </circle>
                ))}

                <text
                  x={Math.max(x1, x0 + 2) + 6}
                  y={y + 3.5}
                  className={cn('font-mono text-[11px]', end.className)}
                >
                  {end.glyph}
                  <title>{end.title}</title>
                </text>
              </g>
            )
          })}
        </svg>
      </div>

      {/* Focus tier: the clicked lane's operations, readable, one row at a
          time. This replaces the card wall rather than shrinking it. */}
      {focused !== null && (
        <div className="max-h-48 shrink-0 overflow-auto border-t border-border bg-card/60 px-3 py-2">
          {(() => {
            const lane = model.lanes[focused]
            if (!lane) return null
            return (
              <>
                <div className="mb-1 font-mono text-[11px] text-foreground">
                  {laneLabel(lane)}
                  <span className="text-muted-foreground">
                    {' '}
                    — born #{lane.bornAtSequence} as {lane.bornAs} · {lane.ticks.length} operations ·{' '}
                    {endMark(lane).title}
                  </span>
                </div>
                <ol className="space-y-0.5">
                  {lane.ticks.map((t, i) => (
                    <li
                      key={`${t.node.id}-${i}`}
                      className="flex items-baseline gap-2 font-mono text-[11px]"
                    >
                      <span className="w-10 shrink-0 tabular-nums text-muted-foreground/70">
                        #{t.node.sequence_number}
                      </span>
                      <span className="min-w-0 flex-1 text-foreground/85">
                        {t.node.operation_type}
                      </span>
                      <span className="shrink-0 text-muted-foreground">{t.entity}</span>
                      <span className="shrink-0 text-muted-foreground/60">{t.node.author}</span>
                    </li>
                  ))}
                </ol>
              </>
            )
          })()}
        </div>
      )}
    </div>
  )
}
