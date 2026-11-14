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

// Constraints: the relation drawn, in yellow, on ink geometry.

export const HorizontalIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 12h16" stroke={Y} strokeWidth="2.4" />
    <text x="12" y="9" fontSize="7" textAnchor="middle" fill="currentColor" stroke="none" fontWeight="700">H</text>
  </Svg>
)

export const VerticalIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M12 4v16" stroke={Y} strokeWidth="2.4" />
    <text x="17.5" y="14" fontSize="7" textAnchor="middle" fill="currentColor" stroke="none" fontWeight="700">V</text>
  </Svg>
)

export const CoincidentIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 18L12 12M12 12l8-7" />
    <circle cx="12" cy="12" r="2.6" fill={Y} stroke="currentColor" strokeWidth="1.2" />
  </Svg>
)

export const OnCurveIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M3 17C8 6 16 6 21 17" />
    <circle cx="12" cy="9.2" r="2.4" fill={Y} stroke="currentColor" strokeWidth="1.2" />
  </Svg>
)

export const MidpointIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M3 12h18" />
    <path d="M3 9v6M21 9v6" strokeWidth="1" />
    <circle cx="12" cy="12" r="2.4" fill={Y} stroke="currentColor" strokeWidth="1.2" />
  </Svg>
)

export const ParallelIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M6 19L14 5M11 19L19 5" stroke={Y} strokeWidth="2.2" />
  </Svg>
)

export const PerpendicularIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M5 19h14M8 19V5" stroke={Y} strokeWidth="2.2" />
    <path d="M8 15h4v4" strokeWidth="1" />
  </Svg>
)

export const EqualIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M5 9h14M5 15h14" stroke={Y} strokeWidth="2.4" />
  </Svg>
)

export const TangentIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="12" cy="13" r="5.5" />
    <path d="M3 7.5h18" stroke={Y} strokeWidth="2.2" />
  </Svg>
)

export const ConcentricIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="12" cy="12" r="8" />
    <circle cx="12" cy="12" r="4" stroke={Y} strokeWidth="2" />
    <Dot x={12} y={12} />
  </Svg>
)

export const SymmetricIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M12 3v18" strokeDasharray="2 2" />
    <circle cx="6" cy="12" r="2.4" fill={Y} stroke="currentColor" strokeWidth="1.2" />
    <circle cx="18" cy="12" r="2.4" fill={Y} stroke="currentColor" strokeWidth="1.2" />
  </Svg>
)

export const DimensionIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 16h16" stroke={Y} strokeWidth="1.8" />
    <path d="M4 13v6M20 13v6" />
    <path d="M7 14l-3 2 3 2M17 14l3 2-3 2" strokeWidth="1.2" />
    <text x="12" y="10" fontSize="7" textAnchor="middle" fill="currentColor" stroke="none" fontWeight="700">40</text>
  </Svg>
)

// Modify

export const TrimIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 12h5M15 12h5" stroke={Y} strokeWidth="2.2" />
    <path d="M9 12h6" strokeDasharray="1.4 1.6" />
    <path d="M9 5v14M15 5v14" />
  </Svg>
)

export const ExtendIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M3 12h9" />
    <path d="M12 12h6" stroke={Y} strokeWidth="2.2" />
    <path d="M14 9l3 3-3 3" stroke={Y} strokeWidth="1.4" />
    <path d="M20 5v14" />
  </Svg>
)

export const SketchFilletIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M5 20V12" />
    <path d="M12 5h7" />
    <path d="M5 12a7 7 0 0 1 7-7" stroke={Y} strokeWidth="2.2" />
  </Svg>
)

export const SketchChamferIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M5 20v-9M11 5h8" />
    <path d="M5 11l6-6" stroke={Y} strokeWidth="2.2" />
  </Svg>
)

export const OffsetIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <rect x="7" y="7" width="10" height="10" rx="1" />
    <rect x="3.5" y="3.5" width="17" height="17" rx="2.5" stroke={Y} strokeWidth="1.8" />
  </Svg>
)

export const MirrorIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M12 3v18" strokeDasharray="2 2" />
    <path d="M9 7L4 17h5z" />
    <path d="M15 7l5 10h-5z" stroke={Y} strokeWidth="1.8" />
  </Svg>
)

export const LinearPatternIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="5" cy="12" r="2.5" />
    <circle cx="12" cy="12" r="2.5" stroke={Y} strokeWidth="1.8" />
    <circle cx="19" cy="12" r="2.5" stroke={Y} strokeWidth="1.8" />
  </Svg>
)

export const CircularPatternIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <Dot x={12} y={12} />
    <circle cx="12" cy="4.5" r="2.2" />
    <circle cx="19" cy="15" r="2.2" stroke={Y} strokeWidth="1.8" />
    <circle cx="5" cy="15" r="2.2" stroke={Y} strokeWidth="1.8" />
  </Svg>
)

export const FixIcon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M4 20h16" />
    <path d="M6 20l2-3M10 20l2-3M14 20l2-3M18 20l2-3" strokeWidth="1" />
    <path d="M12 16V8" />
    <circle cx="12" cy="7" r="3" fill={Y} stroke="currentColor" strokeWidth="1.2" />
  </Svg>
)
