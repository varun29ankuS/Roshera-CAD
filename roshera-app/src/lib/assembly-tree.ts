/**
 * The PRODUCT TREE of imported assemblies, for the model browser.
 *
 * Every part an assembly placed carries an `assembly` occurrence from the
 * kernel (source file, the assemblies above it, the part it is a use of, and
 * which use). This folds those into the tree a CAD user expects:
 *
 *   file → sub-assemblies (in path order) → parts
 *
 * - A file whose parts all sit under ONE top assembly shows that assembly's
 *   contents directly under the file row (the top assembly IS the file —
 *   repeating it as a second level is noise).
 * - A part used more than once in the same place folds into one `×N` group
 *   row whose children are the individual uses.
 * - Names sort naturally (`washer 2` before `washer 10`).
 * - Every node knows the ids of all parts beneath it and how many are sound,
 *   unsound, or not yet certified, so a row can hover / hide / summarise its
 *   whole subtree.
 *
 * Pure module (no store, no React) so it runs under `node --test`.
 */

/** Where an imported part sits in its file's product structure. */
export interface AssemblyOccurrence {
  source: string
  path: string[]
  part: string
  occurrence: string | null
}

/** The fields of a scene object the tree reads. */
export interface TreeInput {
  id: string
  name: string
  sound?: boolean | null
  assembly?: AssemblyOccurrence | null
  /** Set for a use whose geometry did NOT import: the reason. Such an entry
   *  has no mesh; it is a row so the gap is visible, nothing more. */
  missing?: string
}

interface Tally {
  /** Stable key, unique across the whole tree. */
  key: string
  name: string
  /** Every part id beneath this node that EXISTS (has a mesh). */
  partIds: string[]
  partCount: number
  soundCount: number
  unsoundCount: number
  /** Uses beneath this node whose geometry did not import. */
  missingCount: number
}

export interface PartNode extends Tally {
  kind: 'part'
  id: string
  sound: boolean | null
  /** Why this use did not import (absent for parts that did). */
  missing?: string
}

export interface GroupNode extends Tally {
  kind: 'group'
  /** How many uses fold into this row. */
  count: number
  children: PartNode[]
}

export interface ContainerNode extends Tally {
  kind: 'file' | 'assembly'
  children: AsmNode[]
}

export type AsmNode = ContainerNode | GroupNode | PartNode

export interface AssemblyTree {
  roots: ContainerNode[]
  /** Ids of objects no assembly placed (drawn by the rest of the browser). */
  rest: string[]
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })
const byName = (a: { name: string }, b: { name: string }) => collator.compare(a.name, b.name)

/** Mutable builder level: sub-assemblies by name, parts by part name. */
interface Level {
  assemblies: Map<string, Level>
  parts: Map<string, TreeInput[]>
}
const newLevel = (): Level => ({ assemblies: new Map(), parts: new Map() })

export function buildAssemblyTree(objects: Iterable<TreeInput>): AssemblyTree {
  const files = new Map<string, Level>()
  const rest: string[] = []
  for (const obj of objects) {
    const asm = obj.assembly
    if (!asm) {
      rest.push(obj.id)
      continue
    }
    let level: Level = files.get(asm.source) ?? newLevel()
    files.set(asm.source, level)
    for (const name of asm.path) {
      const next: Level = level.assemblies.get(name) ?? newLevel()
      level.assemblies.set(name, next)
      level = next
    }
    const uses = level.parts.get(asm.part)
    if (uses) uses.push(obj)
    else level.parts.set(asm.part, [obj])
  }

  const roots: ContainerNode[] = []
  for (const [source, level] of files) {
    // The single top assembly IS the file: show its contents directly.
    const body =
      level.parts.size === 0 && level.assemblies.size === 1
        ? [...level.assemblies.values()][0]
        : level
    roots.push(container('file', source, `file:${source}`, body))
  }
  roots.sort(byName)
  return { roots, rest }
}

function container(
  kind: 'file' | 'assembly',
  name: string,
  key: string,
  level: Level,
): ContainerNode {
  const children: AsmNode[] = []
  for (const [childName, child] of level.assemblies) {
    children.push(container('assembly', childName, `${key}/${childName}`, child))
  }
  for (const [partName, uses] of level.parts) {
    if (uses.length === 1) {
      children.push(partNode(uses[0], partName))
    } else {
      const instances = uses
        .map((u) => partNode(u, u.assembly?.occurrence || u.name))
        .sort(byName)
      children.push({
        kind: 'group',
        key: `${key}/${partName}#group`,
        name: partName,
        count: uses.length,
        children: instances,
        ...roll(instances),
      })
    }
  }
  children.sort(byName)
  return { kind, key, name, children, ...roll(children) }
}

function partNode(obj: TreeInput, name: string): PartNode {
  if (obj.missing !== undefined) {
    return {
      kind: 'part',
      key: obj.id,
      id: obj.id,
      name,
      sound: null,
      missing: obj.missing,
      partIds: [],
      partCount: 0,
      soundCount: 0,
      unsoundCount: 0,
      missingCount: 1,
    }
  }
  const sound = obj.sound ?? null
  return {
    kind: 'part',
    key: obj.id,
    id: obj.id,
    name,
    sound,
    partIds: [obj.id],
    partCount: 1,
    soundCount: sound === true ? 1 : 0,
    unsoundCount: sound === false ? 1 : 0,
    missingCount: 0,
  }
}

function roll(nodes: Tally[]): Omit<Tally, 'key' | 'name'> {
  const partIds: string[] = []
  let soundCount = 0
  let unsoundCount = 0
  let missingCount = 0
  for (const n of nodes) {
    for (const id of n.partIds) partIds.push(id)
    soundCount += n.soundCount
    unsoundCount += n.unsoundCount
    missingCount += n.missingCount
  }
  return { partIds, partCount: partIds.length, soundCount, unsoundCount, missingCount }
}
