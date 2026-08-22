/**
 * LANES — a document's history as the lives of its solids, not a list of ops.
 *
 * The lineage endpoint serves an operation DAG: 150 nodes each naming the
 * entities it consumed and produced. Rendered directly that is 150 cards, and
 * the founder's verdict on it was "absolutely abysmal" — correctly, because the
 * question he is asking is "how did this part evolve" and a graph of operations
 * answers "what calls happened".
 *
 * A LANE is one solid's life: born at the op that first output its id, carrying
 * ticks for everything that touched it, ending when a boolean retires it, a
 * delete removes it, or the history stops. A boolean's result CONTINUES the
 * lane of the operand it derives from rather than starting a new one, so ids
 * are intervals along a lane rather than lanes themselves. Without that, a
 * cylinder cut six times shatters into seven one-event stubs, which is the
 * current view with extra steps.
 *
 * ## What the data actually does, measured before any of this was written
 *
 * Every assumption here was checked against the live endpoint, and two of the
 * obvious ones are false:
 *
 * - **Ids are NOT stable identity.** Of 34 ops carrying both inputs and
 *   outputs, ZERO preserved an id — every one minted a fresh solid. The chain
 *   is built from output→input references, never from id equality.
 * - **Ids are RECYCLED.** `solid:0` is produced four times in one document and
 *   `solid:2` five times, because a workspace reset restarts numbering. An id
 *   is therefore not a key; the key is (id, the op that produced it). Treating
 *   `solid:2` as one thing splices unrelated parts into a single lane and draws
 *   a confident lie.
 * - **It is not a forest.** `solid:1` has eight consumers on the live document.
 *   Lanes branch, and the view must not promise a tree it does not have.
 *
 * Two irregularities are surfaced rather than smoothed, because both are real:
 * 44 inputs reference a producer outside the served window (the history is
 * windowed), and 3 ids are re-produced while a previous instance is still
 * live.
 */

/** One node of `GET /api/timeline/lineage/{branch}`. */
export interface LineageNode {
  id: string
  sequence_number: number
  timestamp: string
  operation_type: string
  author: string
  author_kind: string
  inputs: string[]
  outputs: string[]
  deleted: string[]
  linked: boolean
}

/** Why a lane stopped. Every terminus is one of these; none is a guess. */
export type LaneEnd =
  /** Consumed by a boolean — the result carries the lane on. */
  | { kind: 'merged'; intoLane: number; atSequence: number }
  /** Explicitly deleted. */
  | { kind: 'deleted'; atSequence: number }
  /** Still live at the end of the served history. */
  | { kind: 'live' }
  /**
   * Consumed by an op that produced no successor, and not a delete. The lane
   * simply stops with nothing recorded about where it went. Surfaced rather
   * than smoothed: this is the view reporting that the recording contract
   * broke, and quietly drawing it as `live` would hide that.
   */
  | { kind: 'dangling'; atSequence: number }
  /**
   * The id was produced again while this instance was still unconsumed, so
   * this lane's later history is unknowable — a second solid took its name.
   * Three of these exist on the live document. Rendered as a break, never
   * stitched to whatever the reused id did next.
   */
  | { kind: 'shadowed'; atSequence: number }

export interface LaneTick {
  node: LineageNode
  /** The entity id this lane was carrying when the op touched it. */
  entity: string
  /** True when this op handed the lane a new id (every boolean does). */
  rebirth: boolean
}

export interface Lane {
  /** Stable index for layout and keys. Not a solid id — those recycle. */
  index: number
  /** The id this lane was born with, for a label when nothing better exists. */
  bornAs: string
  /** The id it is carrying now, which a rename may later replace. */
  currentEntity: string
  /**
   * A human name, ONLY if `part_rename` recorded one. Never synthesised: an
   * unnamed solid stays `solid:15`, because it was not called anything.
   */
  name: string | null
  ticks: LaneTick[]
  bornAtSequence: number
  end: LaneEnd
  /** Lanes that merged INTO this one, for drawing the joins. */
  absorbed: number[]
  /**
   * The lane's first reference had no producer in the served window, so it
   * begins mid-story. Drawn entering from the edge rather than as a birth —
   * 44 references on the live document are like this.
   */
  enteredFromBeforeWindow: boolean
}

export interface LaneModel {
  lanes: Lane[]
  /** Ops that recorded no entity refs at all. They touch no lane, by design. */
  unattached: LineageNode[]
  /** Every op appears exactly once across lanes + unattached; this proves it. */
  accounted: { ops: number; onLanes: number; unattached: number }
}

const isSolid = (ref: string) => ref.startsWith('solid:')

/**
 * Build lanes from the raw node list.
 *
 * The walk is strictly in sequence order, and an input resolves to the MOST
 * RECENT prior production of that id — which is the only resolution that
 * survives id recycling. An input with no prior production is not dropped and
 * not invented: it opens a lane flagged `enteredFromBeforeWindow`.
 */
export function buildLanes(nodes: LineageNode[]): LaneModel {
  const ordered = [...nodes].sort((a, b) => a.sequence_number - b.sequence_number)
  const lanes: Lane[] = []
  const unattached: LineageNode[] = []

  /** Live entity id → the lane currently carrying it. */
  const carrier = new Map<string, Lane>()

  const openLane = (entity: string, node: LineageNode, fromBeforeWindow: boolean): Lane => {
    const lane: Lane = {
      index: lanes.length,
      bornAs: entity,
      currentEntity: entity,
      name: null,
      ticks: [],
      bornAtSequence: node.sequence_number,
      end: { kind: 'live' },
      absorbed: [],
      enteredFromBeforeWindow: fromBeforeWindow,
    }
    lanes.push(lane)
    carrier.set(entity, lane)
    return lane
  }

  let onLanes = 0

  for (const node of ordered) {
    const ins = (node.inputs ?? []).filter(isSolid)
    const outs = (node.outputs ?? []).filter(isSolid)
    const dels = (node.deleted ?? []).filter(isSolid)

    if (node.linked === false || (ins.length === 0 && outs.length === 0 && dels.length === 0)) {
      unattached.push(node)
      continue
    }
    onLanes++

    // Which lanes does this op touch?
    const touched: Lane[] = []
    for (const ref of ins) {
      const lane = carrier.get(ref)
      if (lane) {
        touched.push(lane)
      } else {
        // No producer in the window. The lane starts here, mid-story.
        touched.push(openLane(ref, node, true))
      }
    }

    // The lane that CONTINUES is the first operand — the wire does not say
    // which operand was the base, so "first" is a stated convention rather
    // than a claim about intent. Everything else merges into it.
    const survivor = touched[0] ?? null

    for (const lane of touched) {
      lane.ticks.push({ node, entity: lane.currentEntity, rebirth: false })
      carrier.delete(lane.currentEntity)
      if (lane !== survivor && survivor) {
        lane.end = { kind: 'merged', intoLane: survivor.index, atSequence: node.sequence_number }
        survivor.absorbed.push(lane.index)
      }
    }

    for (const ref of dels) {
      const lane = carrier.get(ref)
      if (lane) {
        lane.ticks.push({ node, entity: ref, rebirth: false })
        lane.end = { kind: 'deleted', atSequence: node.sequence_number }
        carrier.delete(ref)
      }
    }

    // A lane consumed with NO successor produced ends here. Without this the
    // lane stayed marked `live` and the view claimed 70 living solids on a
    // document whose history is 68 deletes — the ending was simply never
    // written, because the terminus is decided by what comes AFTER the op.
    if (outs.length === 0 && survivor && ins.length > 0) {
      const isDelete = node.operation_type.includes('delete')
      survivor.end = isDelete
        ? { kind: 'deleted', atSequence: node.sequence_number }
        : { kind: 'dangling', atSequence: node.sequence_number }
    }

    for (const ref of outs) {
      // An id produced while a previous instance is still live is a genuine
      // collision — the older lane's future becomes unknowable and is broken
      // rather than stitched onto a different solid wearing its name.
      const displaced = carrier.get(ref)
      if (displaced && displaced !== survivor) {
        displaced.end = { kind: 'shadowed', atSequence: node.sequence_number }
        carrier.delete(ref)
      }
      if (survivor) {
        survivor.currentEntity = ref
        survivor.ticks.push({ node, entity: ref, rebirth: true })
        carrier.set(ref, survivor)
      } else {
        const born = openLane(ref, node, false)
        born.ticks.push({ node, entity: ref, rebirth: true })
      }
    }
  }

  return {
    lanes,
    unattached,
    accounted: { ops: ordered.length, onLanes, unattached: unattached.length },
  }
}

/**
 * Checkpoint spans as ERAS — the band a lane passes through, not a per-node
 * title.
 *
 * The current view titles every card with the checkpoint it fell under, which
 * is why thirty cards read `Create a single cylinder with radius 25 mm and
 * heig…`. A checkpoint is not an event; it is a stretch of time, and a stretch
 * of time is drawn once.
 */
export interface Era {
  name: string
  fromSequence: number
  toSequence: number
}

export function erasFromCheckpoints(
  checkpoints: Array<{ name: string; sequence_number?: number }>,
  lastSequence: number,
): Era[] {
  const marks = checkpoints
    .filter((c) => typeof c.sequence_number === 'number')
    .sort((a, b) => (a.sequence_number ?? 0) - (b.sequence_number ?? 0))
  return marks.map((c, i) => ({
    name: c.name,
    fromSequence: c.sequence_number ?? 0,
    toSequence: marks[i + 1]?.sequence_number ?? lastSequence,
  }))
}
