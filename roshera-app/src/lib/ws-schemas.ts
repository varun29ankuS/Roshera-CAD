/**
 * Zod schemas for every `ServerMessage` variant the WebSocket bridge
 * accepts.
 *
 * The bridge is the trust boundary between the network and the scene
 * store. Without runtime validation a malformed frame — a backend bug,
 * a partial deploy, an unrelated WebSocket landing on the same port —
 * would either flow into Zustand as `unknown` and surface later as a
 * confusing `undefined.id` crash, or worse, silently corrupt the
 * scene. The schemas below are validated once on receipt; downstream
 * code consumes the inferred discriminated-union type and never sees
 * `as` casts.
 *
 * # Update protocol
 * When backend adds or changes a `ServerMessage` variant, add the
 * matching schema to `serverMessageSchema` below in the *same* commit.
 * Drift here is the failure mode this file exists to prevent: an
 * unknown variant is dropped at the boundary (logged in dev), so the
 * frontend stays usable while the schema is updated.
 */

import { z } from 'zod'

// ─── Primitive shapes (mirrors `protocol.ts` interfaces) ──────────────

const tuple3 = z.tuple([z.number(), z.number(), z.number()])
const tuple4 = z.tuple([z.number(), z.number(), z.number(), z.number()])

export const meshDataSchema = z.object({
  vertices: z.array(z.number()),
  indices: z.array(z.number()),
  normals: z.array(z.number()),
  uvs: z.array(z.number()).optional(),
  colors: z.array(z.number()).optional(),
  // Per-triangle B-Rep `FaceId` array. Length = `indices.length / 3`.
  // Frontend uses it to map a Three.js raycast hit (which gives a
  // triangle index) back to a kernel face — that's what unlocks
  // Fusion-style face picking and face-extrude.
  face_ids: z.array(z.number()).optional(),
})

export const analyticalGeometrySchema = z.object({
  solid_id: z.number(),
  primitive_type: z.string(),
  parameters: z.record(z.string(), z.number()),
  properties: z.object({
    volume: z.number(),
    surface_area: z.number(),
    bounding_box: z.object({
      min: tuple3,
      max: tuple3,
    }),
    center_of_mass: tuple3,
  }),
})

export const materialPropertiesSchema = z.object({
  diffuse_color: tuple4,
  metallic: z.number(),
  roughness: z.number(),
  emission: tuple3,
  name: z.string(),
})

export const transform3DSchema = z.object({
  translation: tuple3,
  rotation: tuple4,
  scale: tuple3,
})

export const cadObjectSchema = z.object({
  id: z.string(),
  name: z.string(),
  mesh: meshDataSchema,
  analytical_geometry: analyticalGeometrySchema.optional(),
  transform: transform3DSchema,
  material: materialPropertiesSchema,
  visible: z.boolean(),
  locked: z.boolean(),
  parent: z.string().optional(),
  children: z.array(z.string()),
  metadata: z.record(z.string(), z.unknown()),
  created_at: z.number(),
  modified_at: z.number(),
})

const subElementSchema = z.object({
  type: z.enum(['face', 'edge', 'vertex']),
  index: z.number(),
})

// ─── Discriminated union of every accepted ServerMessage variant ────

export const serverMessageSchema = z.discriminatedUnion('type', [
  // First frame the server sends after a successful WebSocket upgrade.
  // Backend variant is unflattened (`{ "type": "Welcome", "connection_id":
  // …, "server_version": …, "capabilities": [...] }`) — i.e. fields
  // sit at the top level alongside `type` rather than under `payload`.
  // We accept both shapes so a backend rev that nests under `payload`
  // doesn't break the bridge.
  z.object({
    type: z.literal('Welcome'),
    connection_id: z.string().optional(),
    server_version: z.string().optional(),
    capabilities: z.array(z.string()).optional(),
    payload: z
      .object({
        connection_id: z.string().optional(),
        server_version: z.string().optional(),
        capabilities: z.array(z.string()).optional(),
      })
      .optional(),
  }),
  // GeometryUpdate carries a nested `Tessellated` envelope so that
  // the backend can later add other geometry-update flavours
  // (LevelOfDetail, Diff, etc.) without rebroadcasting an existing
  // top-level type.
  z.object({
    type: z.literal('GeometryUpdate'),
    payload: z.object({
      type: z.literal('Tessellated'),
      object: cadObjectSchema,
    }),
  }),
  z.object({
    type: z.literal('ObjectCreated'),
    payload: cadObjectSchema,
  }),
  z.object({
    type: z.literal('ObjectUpdated'),
    payload: cadObjectSchema,
  }),
  z.object({
    type: z.literal('ObjectDeleted'),
    payload: z.object({ id: z.string() }),
  }),
  z.object({
    type: z.literal('SceneSync'),
    payload: z.object({ objects: z.array(cadObjectSchema) }),
  }),
  z.object({
    type: z.literal('SessionUpdate'),
    payload: z.object({ session_id: z.string() }),
  }),
  // Heartbeat reply. Backend echoes the original `timestamp` (and may
  // add diagnostic fields), but the bridge does not consume the
  // payload — RTT is timed at the client. Accept any payload shape.
  z.object({
    type: z.literal('Pong'),
    payload: z.unknown().optional(),
  }),
  z.object({
    type: z.literal('SubElementResult'),
    payload: z.object({
      object_id: z.string(),
      elements: z.array(subElementSchema),
    }),
  }),
  z.object({
    type: z.literal('Error'),
    payload: z.object({ message: z.string() }),
  }),
])

/** The sole `ServerMessage` type the frontend is allowed to consume. */
export type ServerMessage = z.infer<typeof serverMessageSchema>

/**
 * Validate a JSON-parsed value against `serverMessageSchema`. Returns
 * the typed message on success, `null` on failure. In development,
 * the schema error is logged with the offending input so backend /
 * frontend drift surfaces immediately; in production we stay silent
 * and drop the frame so a single bad payload cannot poison the UI.
 */
export function parseServerMessage(raw: unknown): ServerMessage | null {
  const result = serverMessageSchema.safeParse(raw)
  if (result.success) return result.data
  if (import.meta.env.DEV) {
    console.error(
      '[WS] Dropping unrecognised or malformed ServerMessage frame:',
      result.error.issues,
      raw,
    )
  }
  return null
}
