/**
 * Roshera's icon set.
 *
 * Every icon is our own artwork, rendered from a small 3D scene by
 * `scripts/icons/build.py` in one CAD style (light grey part, the operation in
 * yellow, green for motion / confirm, red for delete / refuse, blue for
 * information, white sketch points), and rendered NATIVELY at each size in
 * `ICON_SIZES` — an icon is never a stretched bitmap. `<Icon>` picks the
 * rendered size that matches how big it is drawn, plus the 2x render for
 * high-density screens.
 *
 * The named exports keep the names the app used with its previous icon
 * library, so a call site changes only its import; tools whose meaning is a
 * modelling operation use `ToolIcons` instead (see ToolBar, SketchPanel).
 */
/* eslint-disable react-refresh/only-export-components --
 * Every export here is an icon component, but each is made by `makeIcon`, so
 * the fast-refresh rule cannot recognise them by name; this module holds no
 * component state for fast refresh to preserve. */
import { forwardRef, type ComponentType, type ImgHTMLAttributes } from 'react'
import type { ToolIconName, UiIconName } from './icon-manifest'
import { drawnSize, renderedSize } from './icon-size'

export interface IconProps
  extends Omit<ImgHTMLAttributes<HTMLImageElement>, 'src' | 'srcSet' | 'width' | 'height' | 'color'> {
  /** Drawn size in CSS pixels. When absent, read from a Tailwind `size-*` /
   *  `h-*` / `w-*` class, else 24 (the previous library's default). */
  size?: number | string
  /** Accepted for call-site compatibility; a rendered icon has no stroke. */
  strokeWidth?: number | string
  /** Accepted for call-site compatibility; the icon carries its own colours. */
  color?: string
  absoluteStrokeWidth?: boolean
  title?: string
}

export type IconComponent = ComponentType<IconProps>
/** The type call sites used for "an icon component". */
export type LucideIcon = IconComponent

function iconUrl(family: 'tools' | 'ui', name: string, px: number): string {
  return `${import.meta.env.BASE_URL}icons/${family}/${name}-${px}.png`
}

function withoutStrokeProps(props: IconProps): Omit<IconProps, 'strokeWidth' | 'color' | 'absoluteStrokeWidth'> {
  const out: IconProps = { ...props }
  delete out.strokeWidth
  delete out.color
  delete out.absoluteStrokeWidth
  return out
}

function makeIcon(family: 'tools' | 'ui', name: string, label: string): IconComponent {
  const Icon = forwardRef<HTMLImageElement, IconProps>(function Icon(
    props,
    ref,
  ) {
    // strokeWidth / color / absoluteStrokeWidth are accepted for call-site
    // compatibility and deliberately not forwarded to the <img>.
    const { size, className, title, alt, ...rest } = withoutStrokeProps(props)
    const px = drawnSize(size, className)
    const one = renderedSize(px)
    const two = renderedSize(px * 2)
    const decorative = !title && !alt && !rest['aria-label']
    return (
      <img
        ref={ref}
        src={iconUrl(family, name, one)}
        srcSet={`${iconUrl(family, name, one)} 1x, ${iconUrl(family, name, two)} 2x`}
        width={px}
        height={px}
        alt={alt ?? title ?? ''}
        title={title}
        aria-hidden={decorative ? true : undefined}
        draggable={false}
        decoding="async"
        className={['inline-block shrink-0 select-none object-contain', className].filter(Boolean).join(' ')}
        {...rest}
      />
    )
  })
  Icon.displayName = label
  return Icon
}

/** A modelling-tool icon by name. */
export function toolIcon(name: ToolIconName): IconComponent {
  return makeIcon('tools', name, `ToolIcon(${name})`)
}

/** An interface icon by name. */
export function uiIcon(name: UiIconName): IconComponent {
  return makeIcon('ui', name, `UiIcon(${name})`)
}

/** Every modelling-tool icon, keyed by tool. */
export const ToolIcons = {
  box: toolIcon('box'),
  sphere: toolIcon('sphere'),
  cylinder: toolIcon('cylinder'),
  cone: toolIcon('cone'),
  torus: toolIcon('torus'),
  selectObject: toolIcon('select-object'),
  selectFace: toolIcon('select-face'),
  selectEdge: toolIcon('select-edge'),
  selectVertex: toolIcon('select-vertex'),
  translate: toolIcon('translate'),
  rotate: toolIcon('rotate'),
  mirror: toolIcon('mirror'),
  sketch: toolIcon('sketch'),
  point: toolIcon('point'),
  line: toolIcon('line'),
  polyline: toolIcon('polyline'),
  rectangle: toolIcon('rectangle'),
  circle: toolIcon('circle'),
  extrude: toolIcon('extrude'),
  revolve: toolIcon('revolve'),
  loft: toolIcon('loft'),
  sweep: toolIcon('sweep'),
  union: toolIcon('union'),
  intersect: toolIcon('intersect'),
  subtract: toolIcon('subtract'),
  shell: toolIcon('shell'),
  offset: toolIcon('offset'),
  split: toolIcon('split'),
  draft: toolIcon('draft'),
  linearPattern: toolIcon('linear-pattern'),
  circularPattern: toolIcon('circular-pattern'),
  rectangularPattern: toolIcon('rectangular-pattern'),
  copy: toolIcon('copy'),
  hole: toolIcon('hole'),
  thread: toolIcon('thread'),
  rib: toolIcon('rib'),
  fillet: toolIcon('fillet'),
  chamfer: toolIcon('chamfer'),
  massProperties: toolIcon('mass-properties'),
  section: toolIcon('section'),
  interference: toolIcon('interference'),
  measure: toolIcon('measure'),
  datumPlane: toolIcon('datum-plane'),
} as const satisfies Record<string, IconComponent>

// Interface icons under the names the app already uses.
export const AlertTriangle = uiIcon('alert')
export const ArrowLeft = uiIcon('arrow-left')
export const ArrowRight = uiIcon('arrow-right')
export const Ban = uiIcon('ban')
export const CircleSlash = uiIcon('ban')
export const Bot = uiIcon('bot')
export const Box = uiIcon('box-3d')
export const Check = uiIcon('check')
export const CheckIcon = Check
export const CheckCircle2 = uiIcon('check-circle')
export const ChevronDown = uiIcon('chevron-down')
export const ChevronUp = uiIcon('chevron-up')
export const ChevronRight = uiIcon('chevron-right')
export const ChevronRightIcon = ChevronRight
export const CircleHelp = uiIcon('help')
export const Clock = uiIcon('clock')
export const DraftingCompass = uiIcon('compass')
export const Dot = uiIcon('dot')
export const Droplet = uiIcon('droplet')
export const Eye = uiIcon('eye')
export const EyeOff = uiIcon('eye-off')
export const FileDown = uiIcon('file-down')
export const FileUp = uiIcon('file-up')
export const FileText = uiIcon('file')
export const FlaskConical = uiIcon('flask')
export const GitMerge = uiIcon('git-merge')
export const Grid3x3 = uiIcon('grid')
export const Grip = uiIcon('grip')
export const GripHorizontal = uiIcon('grip-horizontal')
export const Lightbulb = uiIcon('lightbulb')
export const ListChecks = uiIcon('list-checks')
export const Loader2 = uiIcon('loader')
export const Lock = uiIcon('lock')
export const Unlock = uiIcon('unlock')
export const Moon = uiIcon('moon')
export const Sun = uiIcon('sun')
export const MousePointer2 = uiIcon('mouse-pointer')
export const MousePointerClick = uiIcon('mouse-pointer-click')
export const Move3d = uiIcon('move')
export const NotebookPen = uiIcon('notebook')
export const Pause = uiIcon('pause')
export const Play = uiIcon('play')
export const Pencil = uiIcon('pencil')
export const PenLine = uiIcon('pencil')
export const PenTool = uiIcon('pen-tool')
export const Pipette = uiIcon('pipette')
export const Plus = uiIcon('plus')
export const RefreshCw = uiIcon('refresh')
export const RefreshCcw = uiIcon('refresh')
export const RotateCw = uiIcon('rotate-cw')
export const RotateCcw = uiIcon('rotate-ccw')
export const Ruler = uiIcon('ruler')
export const Search = uiIcon('search')
export const Send = uiIcon('send')
export const ShieldCheck = uiIcon('shield-check')
export const Square = uiIcon('square')
export const SquareDashed = uiIcon('square-dashed')
export const Tag = uiIcon('tag')
export const Tags = uiIcon('tags')
export const Terminal = uiIcon('terminal')
export const Trash2 = uiIcon('trash')
export const Undo2 = uiIcon('undo')
export const User = uiIcon('user')
export const Wrench = uiIcon('wrench')
export const X = uiIcon('x')
export const XIcon = X
export const XCircle = uiIcon('x-circle')
