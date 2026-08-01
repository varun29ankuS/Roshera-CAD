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
