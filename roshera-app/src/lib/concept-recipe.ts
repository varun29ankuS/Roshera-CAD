/**
 * CONCEPT RECIPES -- the `roshera:concept` card's payload (concept board,
 * spec Roshera-vault/specs/2026-11-27-concept-board-design.md).
 * ======================================================================
 * When an engineer asks an open-ended design question, the agent puts up to
 * three CONCEPTS on the Blackboard before it builds anything. Each concept is
 * a small recipe of signed-distance primitives the app sketches in 3D; the
 * chosen one is then BUILT by the agent through the kernel and certified.
 *
 * The recipe is deliberately the kernel verbs' own vocabulary, so "Build this
 * one" is a field rename, not a conversion (the `concepts` playbook in
 * roshera-mcp gives the mapping):
 *   box      -> create_box      (`center` = centre of the BASE face, width
 *                                along u_axis, depth along v_axis, height
 *                                along u_axis x v_axis)
 *   plate    -> create_box + fillet_edges on the four edges parallel to the
 *               height direction (corner_radius)
 *   cylinder -> create_cylinder (`center` = base centre, extruded along +axis)
 *   cone     -> create_cone     (base_radius at `center`, top_radius at
 *                                center + axis * height)
 *   rib      -> a triangle sketch (origin, origin + leg_a, origin + leg_b)
 *               extruded `thickness` symmetric about its own plane
 *   slot     -> a box plus two end cylinders (a stadium prism), `center` =
 *               centre of its start face, extruded `depth` along `normal`
 * Steps combine in order against the running body: union / subtract /
 * intersect, i.e. boolean union / difference / intersection. `blend` (union
 * only) is a fillet radius on the junction edges.
 *
 * Validation is strict on purpose: an unknown field (a misspelt `raduis`) is
 * refused rather than dropped, axes must be unit and frames orthonormal (the
 * kernel's custom frames require it), and a payload that fails renders as
 * the raw fence with its error -- never as a half sketch.
 */

import { z } from 'zod'

/** The shader's loop bound and the schema's cap are the same number. */
export const MAX_RECIPE_STEPS = 12
export const MAX_CONCEPTS = 3
export const MAX_LOAD_POINTS = 4

const UNIT_TOL = 1e-3

const finite = z.number().finite()
const positive = finite.positive()
const vec3 = z.tuple([finite, finite, finite])

type V3 = [number, number, number]
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
const norm = (a: V3) => Math.sqrt(dot(a, a))
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]

const unitVec3 = vec3.refine((v) => Math.abs(norm(v) - 1) <= UNIT_TOL, {
  message: 'must be a unit vector (length 1)',
})
const nonZeroVec3 = vec3.refine((v) => norm(v) > 1e-9, { message: 'must not be the zero vector' })

/** u and v must be orthogonal: the kernel refuses a non-orthonormal frame. */
const orthogonal = (a: V3, b: V3) => Math.abs(dot(a, b)) <= UNIT_TOL

const boxShape = z
  .object({
    kind: z.literal('box'),
    center: vec3,
    u_axis: unitVec3.default([1, 0, 0]),
    v_axis: unitVec3.default([0, 1, 0]),
    width: positive,
    depth: positive,
    height: positive,
  })
  .strict()

const plateShape = z
  .object({
    kind: z.literal('plate'),
    center: vec3,
    u_axis: unitVec3.default([1, 0, 0]),
    v_axis: unitVec3.default([0, 1, 0]),
    width: positive,
    depth: positive,
    height: positive,
    corner_radius: positive,
  })
  .strict()

const cylinderShape = z
  .object({
    kind: z.literal('cylinder'),
    center: vec3,
    axis: unitVec3.default([0, 0, 1]),
    radius: positive,
    height: positive,
  })
  .strict()

const coneShape = z
  .object({
    kind: z.literal('cone'),
    center: vec3,
    axis: unitVec3.default([0, 0, 1]),
    base_radius: finite.nonnegative(),
    top_radius: finite.nonnegative(),
    height: positive,
  })
  .strict()

const ribShape = z
  .object({
    kind: z.literal('rib'),
    origin: vec3,
    leg_a: nonZeroVec3,
    leg_b: nonZeroVec3,
    thickness: positive,
  })
  .strict()

const slotShape = z
  .object({
    kind: z.literal('slot'),
    center: vec3,
    dir: unitVec3,
    normal: unitVec3,
    length: positive,
    width: positive,
    depth: positive,
  })
  .strict()

const shapeSchema = z
  .discriminatedUnion('kind', [boxShape, plateShape, cylinderShape, coneShape, ribShape, slotShape])
  .superRefine((s, ctx) => {
    switch (s.kind) {
      case 'box':
      case 'plate':
        if (!orthogonal(s.u_axis, s.v_axis))
          ctx.addIssue({ code: 'custom', path: ['v_axis'], message: 'u_axis and v_axis must be orthogonal' })
        if (s.kind === 'plate' && s.corner_radius * 2 >= Math.min(s.width, s.depth))
          ctx.addIssue({
            code: 'custom',
            path: ['corner_radius'],
            message: 'corner_radius must be under half the smaller of width and depth',
          })
        return
      case 'cone':
        if (s.base_radius === 0 && s.top_radius === 0)
          ctx.addIssue({ code: 'custom', path: ['base_radius'], message: 'a cone needs a non-zero base or top radius' })
        return
      case 'rib': {
        const area = norm(cross(s.leg_a, s.leg_b))
        if (area <= 1e-6 * norm(s.leg_a) * norm(s.leg_b))
          ctx.addIssue({ code: 'custom', path: ['leg_b'], message: 'leg_a and leg_b must not be parallel (the rib is a triangle)' })
        return
      }
      case 'slot':
        if (!orthogonal(s.dir, s.normal))
          ctx.addIssue({ code: 'custom', path: ['dir'], message: 'dir and normal must be orthogonal' })
        if (s.length <= s.width)
          ctx.addIssue({ code: 'custom', path: ['length'], message: 'length (end to end) must exceed width' })
        return
      case 'cylinder':
        return
    }
  })

const stepSchema = z
  .object({
    op: z.enum(['union', 'subtract', 'intersect']),
    shape: shapeSchema,
    /** Fillet radius on the junction this union makes (mm). */
    blend: positive.optional(),
  })
  .strict()
  .refine((s) => s.blend === undefined || s.op === 'union', {
    message: 'blend is a fillet on a union junction; subtract and intersect take no blend',
    path: ['blend'],
  })

export const recipeSchema = z
  .object({ steps: z.array(stepSchema).min(1).max(MAX_RECIPE_STEPS) })
  .strict()
  .refine((r) => r.steps[0].op === 'union' && r.steps[0].blend === undefined, {
    message: 'the first step is the starting body: a plain union with no blend',
    path: ['steps', 0],
  })

const conceptNumberSchema = z
  .object({
    label: z.string().min(1).max(48),
    value: z.union([finite, z.string().min(1).max(24)]),
    unit: z.string().max(12).optional(),
    /** How the number was obtained ("beam theory", "hand calc"). Required:
     *  a concept's number is an estimate and says so with its method. */
    basis: z.string().min(1).max(80),
  })
  .strict()

const supportSchema = z.object({ at: vec3, label: z.string().max(40).optional() }).strict()
const loadSchema = z
  .object({
    at: vec3,
    dir: nonZeroVec3,
    magnitude_n: positive.optional(),
    label: z.string().max(40).optional(),
  })
  .strict()

const conceptSchema = z
  .object({
    id: z.string().regex(/^[A-Za-z0-9_-]{1,16}$/, 'id: 1-16 letters, digits, _ or -'),
    title: z.string().min(1).max(60),
    recipe: recipeSchema,
    numbers: z.array(conceptNumberSchema).max(6).optional(),
    /** One short trade-off line ("one setup", "prints only"). */
    note: z.string().max(140).optional(),
    supports: z.array(supportSchema).min(1).max(MAX_LOAD_POINTS).optional(),
    loads: z.array(loadSchema).min(1).max(MAX_LOAD_POINTS).optional(),
  })
  .strict()
  .refine((c) => (c.supports === undefined) === (c.loads === undefined), {
    message: 'loads and supports come together: the load path runs from each load to each support',
    path: ['supports'],
  })

export const conceptSetSchema = z
  .object({
    id: z.string().min(1).max(48).optional(),
    title: z.string().min(1).max(80).optional(),
    /** density in kg/m^3, the part card's unit. */
    material: z.object({ name: z.string().min(1).max(40), density: positive }).strict().optional(),
    concepts: z.array(conceptSchema).min(1).max(MAX_CONCEPTS),
  })
  .strict()
  .refine((s) => new Set(s.concepts.map((c) => c.id)).size === s.concepts.length, {
    message: 'concept ids must be unique',
    path: ['concepts'],
  })

export type ConceptSet = z.infer<typeof conceptSetSchema>
export type Concept = ConceptSet['concepts'][number]
export type Recipe = z.infer<typeof recipeSchema>
export type RecipeStep = Recipe['steps'][number]
export type RecipeShape = RecipeStep['shape']
export type ConceptNumber = z.infer<typeof conceptNumberSchema>
export type ConceptLoad = z.infer<typeof loadSchema>
export type ConceptSupport = z.infer<typeof supportSchema>
