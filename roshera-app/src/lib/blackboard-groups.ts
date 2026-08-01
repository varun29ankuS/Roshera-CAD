import type { BlackboardLine } from '@/stores/blackboard-store'

/**
 * BUILD-STEP GROUPING
 * ====================
 * A machine-authored bookkeeping line eligible for collapsing into the
 * build-step strip (`components/panels/BuildStepStrip.tsx`) — the kernel's
 * own per-operation "Created …" echo (`lib/ws-bridge.ts`'s
 * `dimensionEchoMessage`, `components/layout/ToolBar.tsx`'s direct-create
 * echo), never agent prose or anything the user wrote.
 *
 * ── Why authorship CANNOT be the test ──────────────────────────────────
 * These lines are minted locally as `'system'`, but the backend's author
 * enum accepts only `user`/`agent` (see `lib/blackboard-api.ts`'s
 * `wireAuthor`), so every one of them round-trips back as `'agent'`.
 * Verified against the live store, 2026-08-01: of 38 persisted lines, 36
 * were `agent` and 0 were `system` — including every "Created …" echo.
 * An `author === 'system'` test therefore only holds while the
 * localStorage mirror still carries the local authorship; on a fresh
 * browser, a cleared cache, or any poll where `applyRemoteSnapshot`
 * repaints from backend truth, it silently stops matching and the strip
 * never engages. That is precisely the bug this predicate had.
 *
 * So the test is the TEXT SIGNATURE, which survives the round-trip, and
 * authorship is used only to exclude the human: a user line is content
 * and is never collapsed, however it happens to be worded. The signature
 * is deliberately narrow — a trailing triangle count is bookkeeping the
 * kernel emits and not something an engineer writes in prose, so agent
 * commentary that merely opens with "Created" does not match.
 *
 * The right long-term fix is `System` on the wire enum, so the board
 * stops attributing its own bookkeeping to the agent; that is a backend
 * change and an honesty question in its own right, not a prerequisite
 * for the row being readable.
 */

/** `Created **bore 1/4** — 18 × 18 × 20 mm · 792 tris` (ws-bridge echo). */
const DIMENSION_ECHO = /^Created \*\*[^*]+\*\*.*·\s*\d+\s*tris\s*$/

/** `Created cube (8 verts, 12 tris, 3 ms).` (ToolBar direct-create echo). */
const TOOLBAR_ECHO = /^Created \S+ \(\d+ verts, \d+ tris, [\d.]+ ms\)\.\s*$/

export function isBuildStepLine(line: BlackboardLine): boolean {
  if (line.author === 'user') return false
  return DIMENSION_ECHO.test(line.text) || TOOLBAR_ECHO.test(line.text)
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

/**
 * Longest pause between two steps that still counts as ONE build.
 *
 * List adjacency alone is not sameness. A notebook accumulates across
 * sessions, and once the predicate stopped depending on authorship the
 * persisted history collapsed into a single 33-step breadcrumb spanning
 * several unrelated builds (measured against the live store, 2026-08-01) —
 * which is the same unreadable-row defect in a new costume. Steps the
 * kernel emits inside one build land milliseconds apart; a genuinely new
 * build is separated by however long the engineer took to ask for it. Two
 * minutes sits far above the former and far below the latter.
 */
const BUILD_RUN_GAP_MS = 120_000

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
    if (!isBuildStepLine(line)) {
      flushRun()
      groups.push({ kind: 'line', line })
      continue
    }
    // A long pause since the previous step means a different build, so the
    // open run is closed and this line starts a new one.
    const prev = run[run.length - 1]
    if (prev && line.createdAt - prev.createdAt > BUILD_RUN_GAP_MS) flushRun()
    run.push(line)
  }
  flushRun()

  return groups
}
