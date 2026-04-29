import { STLLoader } from 'three/examples/jsm/loaders/STLLoader.js'
import * as THREE from 'three'
import type { CADMesh, CADObject } from '@/stores/scene-store'

const stlLoader = new STLLoader()

export interface LoadedSTL {
  mesh: CADMesh
  // Bounding box centre and half-extents in object space, used to frame
  // the camera when the demo is loaded.
  center: [number, number, number]
  size: [number, number, number]
}

/// Fetch a binary STL from `url` and return a CAD mesh in the same
/// vertex/index/normals format the scene store expects.
///
/// Throws on network failure or malformed STL.
export async function loadStl(url: string): Promise<LoadedSTL> {
  const response = await fetch(url)
  if (!response.ok) {
    throw new Error(`STL fetch failed (${response.status}) for ${url}`)
  }
  const buffer = await response.arrayBuffer()

  // STLLoader.parse returns a non-indexed BufferGeometry with positions
  // and normals. We keep it non-indexed (indices length 0) — CADMesh's
  // pipeline accepts that path and falls through to computeVertexNormals
  // when normals are absent.
  const geometry = stlLoader.parse(buffer)

  const positionAttr = geometry.getAttribute('position')
  if (!positionAttr) {
    throw new Error(`STL ${url} produced no position attribute`)
  }

  const vertices = new Float32Array(positionAttr.array.length)
  vertices.set(positionAttr.array as ArrayLike<number>)

  let normals: Float32Array
  const normalAttr = geometry.getAttribute('normal')
  if (normalAttr) {
    normals = new Float32Array(normalAttr.array.length)
    normals.set(normalAttr.array as ArrayLike<number>)
  } else {
    normals = new Float32Array(0)
  }

  // Empty index buffer — CADMesh treats indices.length === 0 as
  // non-indexed geometry, which is exactly what STLLoader produces.
  const indices = new Uint32Array(0)

  geometry.computeBoundingBox()
  const bbox = geometry.boundingBox ?? new THREE.Box3()
  const size = new THREE.Vector3()
  const center = new THREE.Vector3()
  bbox.getSize(size)
  bbox.getCenter(center)

  return {
    mesh: { vertices, indices, normals },
    center: [center.x, center.y, center.z],
    size: [size.x, size.y, size.z],
  }
}

/// Wrap a loaded STL in a CADObject suitable for `useSceneStore.addObject`.
export function stlToCadObject(
  id: string,
  name: string,
  loaded: LoadedSTL,
  overrides?: Partial<CADObject>,
): CADObject {
  return {
    id,
    name,
    objectType: 'demo-mesh',
    mesh: loaded.mesh,
    material: {
      color: '#9ca8c4',
      metalness: 0.1,
      roughness: 0.45,
      opacity: 1,
    },
    position: [0, 0, 0],
    rotation: [0, 0, 0],
    scale: [1, 1, 1],
    visible: true,
    locked: false,
    ...overrides,
  }
}
