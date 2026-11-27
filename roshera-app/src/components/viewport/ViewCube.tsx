import { useEffect, useMemo, useRef, useState } from 'react'
import { Line } from '@react-three/drei'
import { useFrame, type ThreeEvent } from '@react-three/fiber'
import * as THREE from 'three'
import { CUBE_FACE_LABELS, CUBE_FACE_TEXT_TURNS, axisEndOn } from '@/lib/view-cube'
import { useViewCubeStore } from '@/stores/view-cube-store'

/**
 * The view cube: the model's orientation, and 26 ways to turn it.
 *
 * Six lettered faces, twelve edges and eight corners; each is a view (see
 * lib/view-cube.ts). The piece under the cursor lights in the hover blue
 * before it is clicked -- the same blue as a part about to be picked. The
 * words are drawn upright for the kernel's Z-up world, in the app's
 * technical face, on a 256 px texture so they stay crisp as the cube turns.
 * From the cube's origin corner the world axes run along its edges: X red,
 * Y green, Z blue.
 *
 * Rendered inside drei's `GizmoHelper`, which turns it with the camera on
 * every rendered frame, including each frame of a fly.
 */

const SIZE = 46 // px across a face, at the helper's 1 unit = 1 px
const PIECE = 0.38 // where edge and corner pieces sit, in cube units
const FONT = '"Share Tech Mono"'

const CORNERS: [number, number, number][] = [
  [1, 1, 1], [1, 1, -1], [1, -1, 1], [1, -1, -1],
  [-1, 1, 1], [-1, 1, -1], [-1, -1, 1], [-1, -1, -1],
]
const EDGES: [number, number, number][] = [
  [1, 1, 0], [1, 0, 1], [1, 0, -1], [1, -1, 0],
  [0, 1, 1], [0, 1, -1], [0, -1, 1], [0, -1, -1],
  [-1, 1, 0], [-1, 0, 1], [-1, 0, -1], [-1, -1, 0],
]

export interface ViewCubeColors {
  face: string
  text: string
  stroke: string
  /** The pre-highlight colour (`--cad-hovered`). */
  hover: string
}

function faceTexture(label: string, quarterTurns: number, colors: ViewCubeColors): THREE.CanvasTexture {
  const canvas = document.createElement('canvas')
  canvas.width = 256
  canvas.height = 256
  const ctx = canvas.getContext('2d')
  if (ctx) {
    ctx.fillStyle = colors.face
    ctx.fillRect(0, 0, 256, 256)
    ctx.translate(128, 128)
    // Canvas angles run clockwise; the table counts anticlockwise turns.
    ctx.rotate((-quarterTurns * Math.PI) / 2)
    ctx.font = `400 ${label.length > 5 ? 54 : 64}px ${FONT}, ui-monospace, monospace`
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = colors.text
    ctx.strokeStyle = colors.text
    // Share Tech Mono has one weight; a stroke over the fill gives the word
    // the weight it needs to stay legible on a foreshortened face (TOP, seen
    // from the isometric view, was the faint one).
    ctx.lineWidth = 3.5
    ctx.lineJoin = 'round'
    ctx.strokeText(label, 0, 4)
    ctx.fillText(label, 0, 4)
  }
  const texture = new THREE.CanvasTexture(canvas)
  texture.colorSpace = THREE.SRGBColorSpace
  texture.anisotropy = 16
  return texture
}

/** A letter at the end of an axis of the triad. */
function letterTexture(letter: string, color: string): THREE.CanvasTexture {
  const canvas = document.createElement('canvas')
  canvas.width = 64
  canvas.height = 64
  const ctx = canvas.getContext('2d')
  if (ctx) {
    ctx.font = `400 46px ${FONT}, ui-monospace, monospace`
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = color
    ctx.strokeStyle = color
    ctx.lineWidth = 2.5
    ctx.strokeText(letter, 32, 35)
    ctx.fillText(letter, 32, 35)
  }
  const texture = new THREE.CanvasTexture(canvas)
  texture.colorSpace = THREE.SRGBColorSpace
  return texture
}

const AXES = [
  { letter: 'X', dir: [1, 0, 0] as const, color: '#d64545' },
  { letter: 'Y', dir: [0, 1, 0] as const, color: '#2e9e4f' },
  { letter: 'Z', dir: [0, 0, 1] as const, color: '#2f6fdb' },
]

export function ViewCube({
  colors,
  onPick,
  mainCamera,
}: {
  /** The viewport's camera (inside the helper, `useThree` gives the helper's
   *  own): the triad is placed and trimmed by it. */
  mainCamera: THREE.Camera
  colors: ViewCubeColors
  /** A piece was clicked: where it sits (the origin for a face) and the
   *  normal of the side that was hit. */
  onPick: (piece: THREE.Vector3, normal: THREE.Vector3) => void
}) {
  // The words are painted to a canvas, which uses whatever font is ready at
  // that instant: paint again once the technical face has loaded.
  const [fontReady, setFontReady] = useState(() => document.fonts?.check?.(`58px ${FONT}`) ?? true)
  useEffect(() => {
    if (fontReady || !document.fonts?.load) return
    let live = true
    void document.fonts.load(`58px ${FONT}`).then(() => {
      if (live) setFontReady(true)
    })
    return () => {
      live = false
    }
  }, [fontReady])

  const faces = useMemo(
    () => CUBE_FACE_LABELS.map((label, i) => faceTexture(label, CUBE_FACE_TEXT_TURNS[i], colors)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [colors.face, colors.text, fontReady],
  )
  useEffect(() => () => faces.forEach((t) => t.dispose()), [faces])
  const letters = useMemo(
    () => AXES.map((a) => letterTexture(a.letter, a.color)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [fontReady],
  )
  useEffect(() => () => letters.forEach((t) => t.dispose()), [letters])

  // A hovered face is its own texture tinted toward the hover blue: light
  // enough that the word still reads.
  const hoverTint = useMemo(() => new THREE.Color(colors.hover).lerp(new THREE.Color('#ffffff'), 0.45), [colors.hover])
  const outline = useMemo(() => new THREE.EdgesGeometry(new THREE.BoxGeometry(1, 1, 1)), [])
  useEffect(() => () => outline.dispose(), [outline])

  const [hoverFace, setHoverFace] = useState<number | null>(null)
  const [hoverPiece, setHoverPiece] = useState<string | null>(null)

  return (
    <group
      scale={SIZE}
      // The controls beside the cube show themselves while the cube is
      // pointed at (ViewCubeControls).
      onPointerOver={() => useViewCubeStore.getState().setHovered(true)}
      onPointerOut={() => useViewCubeStore.getState().setHovered(false)}
    >
      <mesh
        onPointerMove={(e: ThreeEvent<PointerEvent>) => {
          e.stopPropagation()
          if (e.faceIndex != null) setHoverFace(Math.floor(e.faceIndex / 2))
        }}
        onPointerOut={(e: ThreeEvent<PointerEvent>) => {
          e.stopPropagation()
          setHoverFace(null)
        }}
        onClick={(e: ThreeEvent<MouseEvent>) => {
          e.stopPropagation()
          if (e.face) onPick(new THREE.Vector3(0, 0, 0), e.face.normal)
        }}
      >
        <boxGeometry />
        {faces.map((map, i) => (
          <meshBasicMaterial key={i} attach={`material-${i}`} map={map} color={hoverFace === i ? hoverTint : '#ffffff'} />
        ))}
      </mesh>
      <lineSegments geometry={outline} scale={1.002}>
        <lineBasicMaterial color={colors.stroke} />
      </lineSegments>

      {[...EDGES, ...CORNERS].map((p) => {
        const key = p.join()
        const position = new THREE.Vector3(p[0] * PIECE, p[1] * PIECE, p[2] * PIECE)
        const size = p.map((c) => (c === 0 ? 0.5 : 0.25)) as [number, number, number]
        const on = hoverPiece === key
        return (
          <mesh
            key={key}
            position={position}
            scale={1.02}
            onPointerOver={(e: ThreeEvent<PointerEvent>) => {
              e.stopPropagation()
              setHoverFace(null)
              setHoverPiece(key)
            }}
            onPointerOut={(e: ThreeEvent<PointerEvent>) => {
              e.stopPropagation()
              setHoverPiece((cur) => (cur === key ? null : cur))
            }}
            onClick={(e: ThreeEvent<MouseEvent>) => {
              e.stopPropagation()
              if (e.face) onPick(position, e.face.normal)
            }}
          >
            <boxGeometry args={size} />
            <meshBasicMaterial color={colors.hover} transparent opacity={0.85} visible={on} />
          </mesh>
        )
      })}

      <Triad letters={letters} mainCamera={mainCamera} />
    </group>
  )
}

/** Where the axes start: the cube's origin corner (x-min, y-min, z-min),
 *  a hair outside it so the lines do not fight the cube's own outline. */
const ORIGIN = -0.515
/** An axis runs the cube's edge and a little past it -- little enough that,
 *  seen from the origin corner, its tip and letter stay at the corner of the
 *  face beyond and short of that face's word. */
const AXIS_LENGTH = 1.13
const _dir = new THREE.Vector3()

/**
 * The world axes, ON the cube, the way Fusion and Onshape draw them: from the
 * cube's origin corner along its three edges and a little past the far end,
 * X red, Y green, Z blue, a small letter at each tip. They are part of the
 * cube -- depth-tested against it, so an axis is hidden where it runs behind
 * a face, and since they follow edges they never cross a word. An axis
 * pointing straight at the viewer (Y, in the Front view) would be a dot with
 * a letter on the corner, so it is dropped.
 */
function Triad({ letters, mainCamera }: { letters: THREE.CanvasTexture[]; mainCamera: THREE.Camera }) {
  const axes = useRef<(THREE.Group | null)[]>([])
  useFrame(() => {
    const f = mainCamera.getWorldDirection(_dir)
    AXES.forEach((a, i) => {
      const node = axes.current[i]
      if (node) node.visible = !axisEndOn([a.dir[0], a.dir[1], a.dir[2]], [-f.x, -f.y, -f.z])
    })
  })
  return (
    <group position={[ORIGIN, ORIGIN, ORIGIN]}>
      {AXES.map((a, i) => {
        const end: [number, number, number] = [a.dir[0] * AXIS_LENGTH, a.dir[1] * AXIS_LENGTH, a.dir[2] * AXIS_LENGTH]
        const tip: [number, number, number] = [
          a.dir[0] * (AXIS_LENGTH + 0.1),
          a.dir[1] * (AXIS_LENGTH + 0.1),
          a.dir[2] * (AXIS_LENGTH + 0.1),
        ]
        return (
          <group
            key={a.letter}
            ref={(node) => {
              axes.current[i] = node
            }}
          >
            <Line points={[[0, 0, 0], end]} color={a.color} lineWidth={1.5} />
            <sprite position={tip} scale={0.2}>
              <spriteMaterial map={letters[i]} />
            </sprite>
          </group>
        )
      })}
    </group>
  )
}
