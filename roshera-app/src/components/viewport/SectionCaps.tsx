import { useSceneStore } from '@/stores/scene-store'
import { SectionCap } from './SectionCap'
import { SectionCapHatch } from './SectionCapHatch'

/**
 * Group component that renders every active section cap. The cap set
 * is keyed by parent solid id and may contain multiple entries per
 * solid (a plane that crosses a solid in two disjoint places — for
 * example a plane through both ends of a U-shape — produces a cap
 * per loop). Order within a bucket doesn't matter visually since the
 * caps don't overlap.
 *
 * Hidden caps for invisible parent solids: if the user toggled a solid
 * off in the model tree, we drop its caps too so the cap doesn't
 * float in empty space.
 */
export function SectionCaps() {
  const sectionCaps = useSceneStore((s) => s.sectionCaps)
  const objects = useSceneStore((s) => s.objects)

  if (sectionCaps.size === 0) return null

  const nodes: React.ReactNode[] = []
  for (const [solidId, caps] of sectionCaps) {
    const parent = objects.get(solidId)
    if (parent && !parent.visible) continue
    for (let i = 0; i < caps.length; i++) {
      nodes.push(<SectionCap key={`${solidId}:${i}`} cap={caps[i]} />)
      nodes.push(
        <SectionCapHatch key={`${solidId}:${i}:hatch`} cap={caps[i]} />,
      )
    }
  }

  return <group name="section-caps">{nodes}</group>
}
