import type { BlackboardLine } from '@/stores/blackboard-store'

/**
 * BUILD-STEP GROUPING
 * ====================
 * A machine-authored bookkeeping line eligible for collapsing into the
 * build-step strip (`components/panels/BuildStepStrip.tsx`) — the kernel's
 * own per-operation "Created …" echo (`lib/ws-bridge.ts`'s
 * `dimensionEchoMessage`, `components/layout/ToolBar.tsx`'s direct-create
 * echo), never agent prose or anything the user wrote. `author === 'system'`
 * is checked first and is load-bearing: agent/user lines are the content —
 * they are NEVER collapsed, even one that happens to start with "Created".
 */
export function isBuildStepLine(line: BlackboardLine): boolean {
  return line.author === 'system' && /^Created\s/.test(line.text)
}

export type BlackboardGroup =
  | { kind: 'line'; line: BlackboardLine }
  | { kind: 'build-strip'; lines: BlackboardLine[] }

/**
 * Partition a flat, ordered line list into individual lines and runs of two
 * or more ADJACENT `isBuildStepLine` lines. A lone "Created" line (no run to
 * join) stays a normal line — the strip only earns its keep once there is
 * genuine bookkeeping spam to compact away; one line collapsed into "1
 * step, click to expand" would be a regression, not a fix. Non-adjacent
 * runs (a different line — agent prose, a user message, a non-"Created"
 * system line — breaks the sequence) are NEVER merged into one strip; each
 * run gets its own.
 */
// ── Breadcrumb summary for a build-strip run ───────────────────────────
//
// `BuildStepStrip.tsx` used to render one numbered tick per step — an
// index carries no information a user can read without hovering (Varun,
// 2026-08-01). The fix names the operations instead: fold consecutive
// steps sharing the same OPERATION NAME into `name ×N`. This is never a
// fabricated summary — `name` is read verbatim off the line's own
// "Created **name** — …" text, only stripped of a trailing per-instance
// counter ("bore 1/4" → "bore", "Difference 5" → "Difference") so the
// four bores collapse the same way an engineer reading the log aloud
// would fold them.

const CREATED_NAME_RE = /^Created\s+\*\*(.+?)\*\*/

/** The operation name a build-step line reports it created, verbatim
 *  (minus the markdown bold marks) — never truncated or reworded. */
export function buildStepOpName(line: BlackboardLine): string {
  const match = CREATED_NAME_RE.exec(line.text)
  return (match ? match[1] : line.text.replace(/^Created\s+/, '')).trim()
}

/** Strip a trailing per-instance counter so repeats of the same operation
 *  group together — "bore 1/4"/"bore 2/4" → "bore", "Difference 5"/
 *  "Difference 6" → "Difference". Only a TRAILING " N" or " N/M" token
 *  counts; nothing inside the name is touched, so "M6" or "DN50" (which
 *  contain digits but aren't trailing counters) survive unchanged. */
export function normalizeBuildStepName(name: string): string {
  const stripped = name.replace(/\s+\d+(\/\d+)?$/, '').trim()
  return stripped || name
}

export interface BuildStepSegment {
  /** Normalized, display-ready operation name. */
  name: string
  /** How many consecutive lines this segment folds. */
  count: number
  lines: BlackboardLine[]
}

/** Fold a run of build-step lines into name-grouped segments, in order.
 *  Grouping only merges ADJACENT lines with the same normalized name —
 *  "bore", "bore", "Difference" never merges the two "bore"s across a
 *  "Difference" in between (there isn't one here, but the rule matches
 *  `groupBlackboardLines`'s own adjacency-only contract). */
export function groupBuildStepsByName(lines: BlackboardLine[]): BuildStepSegment[] {
  const segments: BuildStepSegment[] = []
  for (const line of lines) {
    const normalized = normalizeBuildStepName(buildStepOpName(line))
    const last = segments[segments.length - 1]
    if (last && last.name === normalized) {
      last.count += 1
      last.lines.push(line)
    } else {
      segments.push({ name: normalized, count: 1, lines: [line] })
    }
  }
  return segments
}

function segmentLabel(seg: BuildStepSegment): string {
  return seg.count > 1 ? `${seg.name} ×${seg.count}` : seg.name
}

const MAX_BREADCRUMB_CHARS = 60

/** The strip's one-line breadcrumb: full text (for the hover title, never
 *  truncated) and a display text that truncates the MIDDLE — keeping the
 *  first and last operations visible, since those are what orient someone
 *  glancing at the row — once there are more than two segments and the
 *  full breadcrumb would overflow a compact row. */
export function buildStepBreadcrumb(segments: BuildStepSegment[]): {
  full: string
  display: string
} {
  const labels = segments.map(segmentLabel)
  const full = labels.join(' · ')
  if (segments.length <= 2 || full.length <= MAX_BREADCRUMB_CHARS) {
    return { full, display: full }
  }
  return { full, display: `${labels[0]} · … · ${labels[labels.length - 1]}` }
}

export function groupBlackboardLines(lines: BlackboardLine[]): BlackboardGroup[] {
  const groups: BlackboardGroup[] = []
  let run: BlackboardLine[] = []

  const flushRun = () => {
    if (run.length === 0) return
    if (run.length === 1) groups.push({ kind: 'line', line: run[0] })
    else groups.push({ kind: 'build-strip', lines: run })
    run = []
  }

  for (const line of lines) {
    if (isBuildStepLine(line)) {
      run.push(line)
    } else {
      flushRun()
      groups.push({ kind: 'line', line })
    }
  }
  flushRun()

  return groups
}
