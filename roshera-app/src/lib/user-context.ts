/**
 * The live user context: what the person points at, has selected and is
 * looking at, reported to the server so an agent can read it while it works
 * (server: `POST` / `GET /api/agent/user-context`, user_context.rs; the
 * agent's tool: `user_context`).
 *
 * The pure part: what a report says, when one is sent, and what the status
 * bar says the agent can see. `hooks/use-user-context-publisher` feeds it
 * from the stores.
 *
 * What is reported is entities, by kernel id -- never a stream of mouse
 * positions. An id is only sent when it IS a kernel id: a face only when
 * the part's mesh names it, an edge or vertex only once the kernel has
 * answered the pick. With sharing paused the report says so and carries
 * nothing else.
 */

export type EntityKind = 'part' | 'face' | 'edge' | 'vertex'
export type Vec3 = [number, number, number]

export interface EntityRef {
  object_id: string
  kind: EntityKind
  /** The kernel id of the face, edge or vertex; absent when not known. */
  id?: number
  /** Where on the part the cursor is. */
  point?: Vec3
}

export interface CameraView {
  position: Vec3
  direction: Vec3
  up: Vec3
  projection: 'perspective' | 'orthographic'
  preset?: string
}

/** The body of `POST /api/agent/user-context`. */
export interface UserContextReport {
  sharing: boolean
  workspace?: 'part' | 'assembly' | 'drawing' | 'dfm' | 'simulate'
  tool?: string
  pick_filter?: EntityKind
  hover?: EntityRef
  selection?: EntityRef[]
  /** How many things are selected, when more than are listed. */
  selection_total?: number
  feature?: number
  history_step?: number
  camera?: CameraView
  at_ms?: number
}

/** The server's answer to a report. */
export interface PublishAck {
  seq: number
  agent_look: { agent: string; age_ms: number; seq: number } | null
}

/** The most entities one report lists (the server refuses more). */
export const MAX_SELECTION = 256

/** What the stores know, gathered for one report. */
export interface ContextInput {
  sharing: boolean
  workspace: string | null
  tool: string
  /** What a click picks: `object` is a whole part. */
  selectionMode: 'object' | 'face' | 'edge' | 'vertex'
  selectedIds: Iterable<string>
  subElements: readonly {
    objectId: string
    type: 'face' | 'edge' | 'vertex'
    index: number
    /** True once the kernel answered the pick with its own id. */
    resolved?: boolean
  }[]
  /** The part hovered in the viewport or on its tree row. */
  hoveredId: string | null
  /** The nearest part under the cursor, the point on it, and the kernel
   *  face there when the mesh names it. */
  cursor: { objectId: string; point: readonly [number, number, number]; faceId: number | undefined } | null
  /** The kernel face ids a part's mesh carries, if it carries any. */
  faceIdsOf: (objectId: string) => ArrayLike<number> | undefined
  feature: number | null
  historyStep: number | null
  camera: CameraView | null
}

const WORKSPACES = ['part', 'assembly', 'drawing', 'dfm', 'simulate'] as const

function round(v: number, places: number): number {
  const k = 10 ** places
  return Math.round(v * k) / k
}

function roundVec(p: readonly [number, number, number], places = 2): Vec3 {
  return [round(p[0], places), round(p[1], places), round(p[2], places)]
}

function hasFace(faceIds: ArrayLike<number> | undefined, id: number): boolean {
  if (!faceIds) return false
  for (let i = 0; i < faceIds.length; i++) if (faceIds[i] === id) return true
  return false
}

function hoverOf(input: ContextInput): EntityRef | undefined {
  const c = input.cursor
  if (c) {
    const point = roundVec(c.point)
    // Picking edges or vertices: the person means one of those, and which
    // one is not known until the kernel answers a click.
    if (input.selectionMode === 'edge' || input.selectionMode === 'vertex') {
      return { object_id: c.objectId, kind: input.selectionMode, point }
    }
    if (c.faceId !== undefined) return { object_id: c.objectId, kind: 'face', id: c.faceId, point }
    return { object_id: c.objectId, kind: 'part', point }
  }
  return input.hoveredId ? { object_id: input.hoveredId, kind: 'part' } : undefined
}

function selectionOf(input: ContextInput): EntityRef[] {
  const out: EntityRef[] = []
  const withSub = new Set<string>()
  for (const s of input.subElements) {
    withSub.add(s.objectId)
    const known = s.type === 'face' ? hasFace(input.faceIdsOf(s.objectId), s.index) : s.resolved === true
    out.push(known ? { object_id: s.objectId, kind: s.type, id: s.index } : { object_id: s.objectId, kind: s.type })
  }
  // A part with a face, edge or vertex picked on it is selected through
  // that pick; listing the whole part too would say less, not more.
  for (const id of input.selectedIds) if (!withSub.has(id)) out.push({ object_id: id, kind: 'part' })
  return out
}

/** The report for what the stores hold now. Paused: the pause, alone. */
export function buildReport(input: ContextInput, nowMs: number): UserContextReport {
  if (!input.sharing) return { sharing: false, at_ms: nowMs }
  const report: UserContextReport = { sharing: true, tool: input.tool }
  const ws = WORKSPACES.find((w) => w === input.workspace)
  if (ws) report.workspace = ws
  report.pick_filter = input.selectionMode === 'object' ? 'part' : input.selectionMode
  const hover = hoverOf(input)
  if (hover) report.hover = hover
  const selection = selectionOf(input)
  report.selection = selection.slice(0, MAX_SELECTION)
  if (selection.length > MAX_SELECTION) report.selection_total = selection.length
  if (input.feature !== null) report.feature = input.feature
  if (input.historyStep !== null) report.history_step = input.historyStep
  if (input.camera) {
    report.camera = {
      ...input.camera,
      position: roundVec(input.camera.position),
      direction: roundVec(input.camera.direction, 3),
      up: roundVec(input.camera.up, 3),
    }
  }
  report.at_ms = nowMs
  return report
}

/** Whether two reports say the same thing (their clocks aside). */
export function sameReport(a: UserContextReport | null, b: UserContextReport | null): boolean {
  if (a === null || b === null) return a === b
  return JSON.stringify({ ...a, at_ms: 0 }) === JSON.stringify({ ...b, at_ms: 0 })
}

/** The camera alone moving is not a reason to report at once: it rides on
 *  the next report or the next heartbeat. */
function sameButCamera(a: UserContextReport | null, b: UserContextReport | null): boolean {
  if (a === null || b === null) return a === b
  return sameReport({ ...a, camera: undefined }, { ...b, camera: undefined })
}

export interface PublisherOptions {
  /** Send one report; resolves to the server's answer, or null when it
   *  could not be delivered. Never rejects. */
  send: (report: UserContextReport) => Promise<PublishAck | null>
  now: () => number
  schedule: (fn: () => void, ms: number) => unknown
  cancel: (handle: unknown) => void
  /** Called with each answer, and the report it answers. */
  onAck?: (ack: PublishAck, report: UserContextReport) => void
  /** Quiet time after a change before it is sent. */
  settleMs?: number
  /** The longest a change waits while changes keep coming. */
  maxWaitMs?: number
  /** A live report is re-sent this often, so the server can tell an open
   *  app from a closed one. */
  heartbeatMs?: number
}

/**
 * Decides when a report goes out. A change is sent once the context has
 * been still for `settleMs` (a cursor crossing a part sends nothing until
 * it rests), and never later than `maxWaitMs` after the first unsent
 * change. An unchanged live context is re-sent every `heartbeatMs`. One
 * request at a time; while one is in flight the newest report waits and
 * the ones between are dropped -- attention has no backlog.
 */
export class ContextPublisher {
  private readonly send: PublisherOptions['send']
  private readonly now: PublisherOptions['now']
  private readonly schedule: PublisherOptions['schedule']
  private readonly cancel: PublisherOptions['cancel']
  private readonly onAck: PublisherOptions['onAck']
  private readonly settleMs: number
  private readonly maxWaitMs: number
  private readonly heartbeatMs: number
  private latest: UserContextReport | null = null
  private sent: UserContextReport | null = null
  private sentAt = -Infinity
  private firstUnsentAt: number | null = null
  private timer: unknown = null
  private inFlight = false
  private stopped = false

  constructor(options: PublisherOptions) {
    this.send = options.send
    this.now = options.now
    this.schedule = options.schedule
    this.cancel = options.cancel
    this.onAck = options.onAck
    this.settleMs = options.settleMs ?? 150
    this.maxWaitMs = options.maxWaitMs ?? 1000
    this.heartbeatMs = options.heartbeatMs ?? 2000
  }

  /** The stores changed: `report` is the context now. */
  offer(report: UserContextReport): void {
    if (this.stopped) return
    this.latest = report
    if (sameButCamera(report, this.sent)) {
      // Back to what the server already has: nothing is owed.
      if (this.timer !== null && !this.inFlight) {
        this.cancel(this.timer)
        this.timer = null
        this.firstUnsentAt = null
      }
      return
    }
    const now = this.now()
    if (this.firstUnsentAt === null) this.firstUnsentAt = now
    const wait = Math.max(0, Math.min(this.settleMs, this.firstUnsentAt + this.maxWaitMs - now))
    this.arm(wait)
  }

  /** Called on a steady beat with the context now: re-sends a live report
   *  that has not been sent for `heartbeatMs`. A paused app has nothing to
   *  keep alive. */
  heartbeat(report: UserContextReport): void {
    if (this.stopped) return
    this.latest = report
    if (this.timer !== null || this.inFlight) return
    if (!report.sharing && this.sent !== null && !this.sent.sharing) return
    if (this.now() - this.sentAt >= this.heartbeatMs) this.flush()
  }

  /** Stop for good (the app is closing). */
  stop(): void {
    this.stopped = true
    if (this.timer !== null) this.cancel(this.timer)
    this.timer = null
  }

  private arm(ms: number): void {
    if (this.timer !== null) this.cancel(this.timer)
    this.timer = this.schedule(() => {
      this.timer = null
      this.flush()
    }, ms)
  }

  private flush(): void {
    if (this.stopped || this.latest === null) return
    if (this.inFlight) return // sent when the one in flight settles
    const report = this.latest
    this.inFlight = true
    this.firstUnsentAt = null
    void this.send(report).then((ack) => {
      this.inFlight = false
      if (ack) {
        this.sent = report
        this.sentAt = this.now()
        this.onAck?.(ack, report)
      }
      if (this.stopped) return
      // What changed while this was in flight goes next.
      if (this.latest !== null && !sameButCamera(this.latest, report) && this.timer === null) {
        if (this.firstUnsentAt === null) this.firstUnsentAt = this.now()
        this.arm(this.settleMs)
      }
    })
  }
}

/** What the status bar says the agent can see. */
export function agentSeesLabel(
  report: UserContextReport | null,
  nameOf: (objectId: string) => string | null,
): string {
  if (report === null) return 'not shared yet'
  if (!report.sharing) return 'nothing · paused'
  const describe = (e: EntityRef): string => {
    const part = nameOf(e.object_id) ?? 'part'
    if (e.kind === 'part') return part
    return e.id === undefined ? `${part} · ${e.kind === 'edge' ? 'an' : 'a'} ${e.kind}` : `${part} · ${e.kind} ${e.id}`
  }
  if (report.hover) return describe(report.hover)
  const n = report.selection_total ?? report.selection?.length ?? 0
  if (n === 1 && report.selection) return `selected ${describe(report.selection[0])}`
  if (n > 1) return `${n} selected`
  if (report.feature !== undefined) return `feature at step ${report.feature}`
  return 'nothing pointed at'
}

/** An agent's read of the context, as the app keeps it. */
export interface AgentLook {
  agent: string
  /** The app's clock when the agent read. */
  atMs: number
  /** The report it read. */
  seq: number
}

/** How long an agent's read stays shown as "looking". */
export const LOOK_SHOWN_MS = 4000

/** "Claude looked just now" / "Claude looked 12 s ago" / null when no agent
 *  has read the context. */
export function lookLabel(look: AgentLook | null, nowMs: number): string | null {
  if (!look) return null
  const s = Math.max(0, Math.round((nowMs - look.atMs) / 1000))
  if (s < 2) return `${look.agent} looked just now`
  if (s < 60) return `${look.agent} looked ${s} s ago`
  return `${look.agent} looked ${Math.round(s / 60)} min ago`
}

/** Whether the agent is looking now (its read is recent). */
export function isLooking(look: AgentLook | null, nowMs: number): boolean {
  return look !== null && nowMs - look.atMs < LOOK_SHOWN_MS
}
