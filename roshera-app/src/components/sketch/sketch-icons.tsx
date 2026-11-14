/**
 * Sketch tool icons, in the app's tool style: ink outline on a light
 * ground, the thing the tool MAKES in yellow (the feature colour).
 */

const Y = '#F2A516'

function Svg({ size = 20, children }: { size?: number; children: React.ReactNode }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {children}
    </svg>
  )
}

const Dot = ({ x, y, fill = 'currentColor' }: { x: number; y: number; fill?: string }) => (
  <circle cx={x} cy={y} r="1.7" fill={fill} stroke="none" />
)

export const LineIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M5 18L19 6" stroke={Y} strokeWidth="2.2" />
    <Dot x={5} y={18} />
    <Dot x={19} y={6} />
  </Svg>
)

export const CenterlineIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M3 12h3M9 12h2M14 12h3M20 12h1" stroke={Y} strokeWidth="2" />
  </Svg>
)

export const RectIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <rect x="4" y="7" width="16" height="10" stroke={Y} strokeWidth="2" />
    <Dot x={4} y={17} />
    <Dot x={20} y={7} />
  </Svg>
)

export const CenterRectIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <rect x="4" y="7" width="16" height="10" stroke={Y} strokeWidth="2" />
    <Dot x={12} y={12} />
    <path d="M12 12L20 7" strokeDasharray="1.5 1.5" />
  </Svg>
)

export const CircleIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="12" cy="12" r="7.5" stroke={Y} strokeWidth="2" />
    <Dot x={12} y={12} />
    <path d="M12 12h7.5" strokeDasharray="1.5 1.5" />
  </Svg>
)

export const Arc3Icon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 17A9 9 0 0 1 20 17" stroke={Y} strokeWidth="2.2" />
    <Dot x={4} y={17} />
    <Dot x={12} y={8.3} />
    <Dot x={20} y={17} />
  </Svg>
)

export const CenterArcIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M19 13A7 7 0 0 0 12 6" stroke={Y} strokeWidth="2.2" />
    <Dot x={12} y={13} />
    <path d="M12 13V6M12 13h7" strokeDasharray="1.5 1.5" />
  </Svg>
)

export const PolygonIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M12 4l7 4v8l-7 4-7-4V8z" stroke={Y} strokeWidth="2" />
  </Svg>
)

export const SlotIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M8 8h8a4 4 0 0 1 0 8H8a4 4 0 0 1 0-8z" stroke={Y} strokeWidth="2" />
    <Dot x={8} y={12} />
    <Dot x={16} y={12} />
  </Svg>
)

export const SplineIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 17C7 5 12 20 15 10s4-4 5-3" stroke={Y} strokeWidth="2.2" />
    <path d="M4 17L8 6l6 12 6-11" strokeDasharray="1.2 1.6" strokeWidth="1" />
  </Svg>
)

export const PointIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M12 6v3M12 15v3M6 12h3M15 12h3" />
    <circle cx="12" cy="12" r="2.4" fill={Y} stroke="currentColor" strokeWidth="1.2" />
  </Svg>
)

export const SelectIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M6 4l11 6-5 1.6L10 17z" fill={Y} />
  </Svg>
)

export const DeleteIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M5 7h14M10 7V5h4v2M7 7l1 12h8l1-12" />
    <path d="M10 10v6M14 10v6" />
  </Svg>
)

export const ConstructionIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 18L20 6" stroke={Y} strokeWidth="2" strokeDasharray="3 2.5" />
  </Svg>
)

export const UndoIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M9 7L5 11l4 4" />
    <path d="M5 11h9a5 5 0 0 1 0 10h-3" />
  </Svg>
)
