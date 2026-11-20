/**
 * The agent's HANDS on a sheet, shown (agent eyes slice 3): what the Drawing
 * workspace draws when a layout verb lands. The server sends two AgentShow
 * frames per verb (POST /api/drawings/{id}/layout, for an agent's call):
 *
 * 1. ACT (`sheet_act`): every moved thing -- a dimension, a view, a frame --
 *    glides from its old box to its new one over `ACT_GLIDE_MS`, eased; a
 *    faint ghost of the old box stays at the old place and fades over
 *    `GHOST_MS`. The caption says what was done and why, by rule id.
 * 2. JUDGE (`sheet_judge`): each touched space flashes its score change --
 *    green better, red worse -- worst first, staggered; the bar carries the
 *    grade change and the certificate's verdict.
 *
 * Reduced motion: no glide and no fade -- the new box at once, the ghost
 * shown still for its lifetime, the flashes all at once; the same captions.
 * These are the pure pieces; the store (stores/agent-show-store) keeps the
 * frames and the workspace (DrawingsWorkspace) draws them.
 */
import type { SheetRect } from './sheet-look.ts'

/** How long a moved thing takes to glide from its old box to its new one. */
export const ACT_GLIDE_MS = 500
/** How long the ghost of the old box stays (fading, unless reduced motion). */
export const GHOST_MS = 1000
/** The ghost's opacity when it appears. */
export const GHOST_OPACITY = 0.35
/** How long one space's score flash lasts. */
export const JUDGE_FLASH_MS = 1400
/** The delay between one flash and the next, worst space first. */
export const JUDGE_STAGGER_MS = 120
/** Better / worse / unchanged. */
export const JUDGE_GREEN = '#16a34a'
export const JUDGE_RED = '#dc2626'
export const JUDGE_GREY = '#6b7280'

export type MovedKind = 'dimension' | 'view' | 'frame'

/** One moved thing, as the server's act frame states it. */
export interface SheetMove {
  kind: MovedKind
  id: string
  from?: SheetRect | null
  to?: SheetRect | null
}

/** One space's score change, as the server's judge frame states it. */
export interface SheetScore {
  space: string
  before?: number | null
  after?: number | null
  rect?: SheetRect | null
}

const ease = (u: number) => (u < 0.5 ? 4 * u * u * u : 1 - Math.pow(-2 * u + 2, 3) / 2)
const clamp01 = (u: number) => Math.min(1, Math.max(0, u))

/** The box `u` (0..1, already eased) of the way from `a` to `b`. */
export function lerpRect(a: SheetRect, b: SheetRect, u: number): SheetRect {
  const k = clamp01(u)
  return {
    x0: a.x0 + (b.x0 - a.x0) * k,
    y0: a.y0 + (b.y0 - a.y0) * k,
    x1: a.x1 + (b.x1 - a.x1) * k,
    y1: a.y1 + (b.y1 - a.y1) * k,
  }
}

/**
 * Where a moved thing is drawn `t` ms after the act arrived: on its way from
 * the old box to the new one (eased), the new box once the glide is done.
 * A thing that only appeared (no old box) is at its new box; one that went
 * (a removed view: no new box) is drawn at its old box until the glide's end,
 * then not at all (null). Reduced motion: the end state at once.
 */
export function movedBoxAt(m: SheetMove, t: number, reducedMotion: boolean, glideMs: number = ACT_GLIDE_MS): SheetRect | null {
  const from = m.from ?? null
  const to = m.to ?? null
  const done = reducedMotion || glideMs <= 0 || t >= glideMs
  if (!to) return done ? null : from
  if (!from || done) return to
  return lerpRect(from, to, ease(clamp01(t / glideMs)))
}

/**
 * The ghost of the old box `t` ms after the act: its opacity, 0 once its
 * lifetime is over (or when there was no old box to leave). It fades
 * linearly; under reduced motion it stays at `GHOST_OPACITY` for its
 * lifetime and then goes, with no fade.
 */
export function ghostOpacity(m: SheetMove, t: number, reducedMotion: boolean, lifeMs: number = GHOST_MS): number {
  if (!m.from || t < 0 || t >= lifeMs) return 0
  if (reducedMotion) return GHOST_OPACITY
  return GHOST_OPACITY * (1 - t / lifeMs)
}

/** True while anything of the act is still changing or showing. */
export function actRunning(t: number): boolean {
  return t < Math.max(ACT_GLIDE_MS, GHOST_MS)
}

export type Verdict = 'better' | 'worse' | 'same' | 'new' | 'gone'

/** What a space's score did. */
export function scoreVerdict(s: SheetScore): Verdict {
  const b = typeof s.before === 'number' ? s.before : null
  const a = typeof s.after === 'number' ? s.after : null
  if (b === null && a === null) return 'same'
  if (b === null) return 'new'
  if (a === null) return 'gone'
  if (Math.abs(a - b) < 1e-9) return 'same'
  return a > b ? 'better' : 'worse'
}

/** A space's flash: where, what colour, the label, and when it starts. */
export interface JudgeFlash {
  space: string
  rect: SheetRect
  verdict: Verdict
  color: string
  label: string
  delayMs: number
}

const fmt = (v: number | null | undefined) => (typeof v === 'number' ? v.toFixed(2) : '–')

/**
 * How long a judgement waits before it flashes: JUDGE comes AFTER the act,
 * so a judge frame that lands while its act is still gliding (the route sends
 * the two within milliseconds) waits out the rest of the glide. Nothing to
 * wait for under reduced motion, or with no act of that sheet running
 * (`sinceActMs` null).
 */
export function judgeDelayMs(sinceActMs: number | null, reducedMotion: boolean): number {
  if (reducedMotion || sinceActMs === null || !Number.isFinite(sinceActMs)) return 0
  return Math.max(0, ACT_GLIDE_MS - Math.max(0, sinceActMs))
}

/**
 * The bar's caption once the judgement lands on an act: what was done and why
 * (by rule id), then the verdict -- the act's caption must not be replaced by
 * the judge's a few milliseconds after it appeared.
 */
export function actJudgeCaption(act: string | null, judge: string): string {
  return act ? `${act} — ${judge}` : judge
}

/**
 * The flashes a judge frame makes, WORST FIRST: by the score after (before,
 * for a space that went), lowest first; a space with no box on the paper is
 * not drawn. Green where it improved, red where it got worse (a space that
 * went is green -- its findings went with it -- one that appeared red when
 * it brought findings). The first starts `afterMs` in (the rest of its act's
 * glide: `judgeDelayMs`), the rest staggered by `JUDGE_STAGGER_MS`; reduced
 * motion: all at once.
 */
export function judgeFlashes(spaces: SheetScore[], reducedMotion: boolean, afterMs: number = 0): JudgeFlash[] {
  const key = (s: SheetScore) =>
    typeof s.after === 'number' ? s.after : typeof s.before === 'number' ? s.before : 1
  return [...spaces]
    .filter((s): s is SheetScore & { rect: SheetRect } => !!s.rect)
    .sort((a, b) => key(a) - key(b) || a.space.localeCompare(b.space))
    .map((s, i) => {
      const verdict = scoreVerdict(s)
      const better = verdict === 'better' || verdict === 'gone' || (verdict === 'new' && (s.after ?? 1) >= 1)
      const worse = verdict === 'worse' || (verdict === 'new' && (s.after ?? 1) < 1)
      return {
        space: s.space,
        rect: s.rect,
        verdict,
        color: better ? JUDGE_GREEN : worse ? JUDGE_RED : JUDGE_GREY,
        label: `${fmt(s.before)} → ${fmt(s.after)}`,
        delayMs: reducedMotion ? 0 : Math.max(0, afterMs) + i * JUDGE_STAGGER_MS,
      }
    })
}

/** A flash's opacity `t` ms after the judge arrived (0 before its turn and
 *  after its time); reduced motion holds it steady, no pulse. */
export function flashOpacity(f: JudgeFlash, t: number, reducedMotion: boolean, lifeMs: number = JUDGE_FLASH_MS): number {
  const local = t - f.delayMs
  if (local < 0 || local >= lifeMs) return 0
  if (reducedMotion) return 1
  // In fast, hold, out over the last third.
  const u = local / lifeMs
  if (u < 0.1) return u / 0.1
  if (u > 2 / 3) return (1 - u) * 3
  return 1
}

/** True while any flash is still showing. */
export function judgeRunning(flashes: JudgeFlash[], t: number): boolean {
  return flashes.some((f) => t < f.delayMs + JUDGE_FLASH_MS)
}
