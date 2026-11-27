/**
 * CONCEPT GLSL -- the fragment shader that sketches a concept.
 * ============================================================
 * The scene function is GENERATED from the recipe, out of the same prepared
 * primitives concept-sdf.ts evaluates (`prepareRecipe`), and the library
 * below is the GLSL twin of concept-sdf.ts's `primitiveSdf` / `combine`.
 * Change a formula in one and change it in the other.
 *
 * The shader sphere-traces an orthographic view of the field and shades it
 * like a pencil sketch (three tones, hatching in the darker two, ink on the
 * silhouette and on creases). The point of view (`uPov`) recolours it with
 * the rules concept-pov.ts holds and tests:
 *   1 load path  -- material within the load path in ink, the rest faded;
 *   2 maker mill -- surface reachable from +Z plain, only from -Z hatched,
 *                   from neither filled in the caution colour;
 *   3 maker FDM  -- overhangs (> 45 degrees past vertical, downward, not on
 *                   the bed) filled in the caution colour.
 * `uReveal` (0..1) hides the rows below it: the sketch prints top to bottom
 * in a few steps, like the rest of the board.
 */

import type { Recipe } from './concept-recipe.ts'
import { prepareRecipe, type Prepared, type PreparedStep, type Vec3 } from './concept-sdf.ts'
import { LOAD_PATH_ON, OVERHANG_COS } from './concept-pov.ts'
import { MAX_LOAD_POINTS } from './concept-recipe.ts'

/** A GLSL ES float literal: always with a decimal point, never NaN. */
export function glslFloat(n: number): string {
  if (!Number.isFinite(n)) throw new Error(`not a finite number: ${n}`)
  if (Number.isInteger(n)) return `${n === 0 ? 0 : n}.0`
  let s = String(n)
  if (s.includes('e')) s = n.toFixed(12).replace(/0+$/, '')
  return s.includes('.') ? s : `${s}.0`
}

const v3 = (v: Vec3) => `vec3(${glslFloat(v[0])}, ${glslFloat(v[1])}, ${glslFloat(v[2])})`
const f = glslFloat

function primitiveCall(q: Prepared): string {
  switch (q.kind) {
    case 'box':
      return `sdBox(p, ${v3(q.c)}, ${v3(q.u)}, ${v3(q.v)}, ${v3(q.n)}, ${v3(q.h)})`
    case 'plate':
      return `sdPlate(p, ${v3(q.c)}, ${v3(q.u)}, ${v3(q.v)}, ${v3(q.n)}, ${v3(q.h)}, ${f(q.r)})`
    case 'cylinder':
      return `sdCylinder(p, ${v3(q.a)}, ${v3(q.axis)}, ${f(q.r)}, ${f(q.len)})`
    case 'cone':
      return `sdCone(p, ${v3(q.a)}, ${v3(q.b)}, ${f(q.ra)}, ${f(q.rb)})`
    case 'rib':
      return `sdRib(p, ${v3(q.o)}, ${v3(q.u)}, ${v3(q.v)}, ${v3(q.n)}, ${f(q.p1x)}, vec2(${f(q.p2[0])}, ${f(q.p2[1])}), ${f(q.ht)})`
    case 'slot':
      return `sdSlot(p, ${v3(q.c)}, ${v3(q.u)}, ${v3(q.v)}, ${v3(q.n)}, ${f(q.hl)}, ${f(q.r)}, ${f(q.depth)})`
  }
}

function combineCall(s: PreparedStep): string {
  switch (s.op) {
    case 'union':
      return s.blend > 0 ? `opUnionRound(d, s, ${f(s.blend)})` : 'opUnion(d, s)'
    case 'subtract':
      return 'opSubtract(d, s)'
    case 'intersect':
      return 'opIntersect(d, s)'
  }
}

/** `float sceneSdf(vec3 p)` for this recipe. */
export function sceneGlsl(recipe: Recipe): string {
  const steps = prepareRecipe(recipe)
  const lines = [`float sceneSdf(vec3 p) {`, `  float d = ${primitiveCall(steps[0].prim)};`, `  float s;`]
  for (const s of steps.slice(1)) {
    lines.push(`  s = ${primitiveCall(s.prim)};`)
    lines.push(`  d = ${combineCall(s)};`)
  }
  lines.push(`  return d;`, `}`)
  return lines.join('\n')
}

/** The GLSL twin of concept-sdf.ts's primitive fields and combines. */
export const GLSL_SDF_LIBRARY = `
float extrudeField(float d2, float z, float hz) {
  vec2 w = vec2(d2, abs(z) - hz);
  return min(max(w.x, w.y), 0.0) + length(max(w, 0.0));
}
float sdBox(vec3 p, vec3 c, vec3 u, vec3 v, vec3 n, vec3 h) {
  vec3 d = p - c;
  vec3 q = abs(vec3(dot(d, u), dot(d, v), dot(d, n))) - h;
  return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0);
}
float sdPlate(vec3 p, vec3 c, vec3 u, vec3 v, vec3 n, vec3 h, float r) {
  vec3 d = p - c;
  vec2 q = abs(vec2(dot(d, u), dot(d, v))) - (h.xy - r);
  float d2 = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
  return extrudeField(d2, dot(d, n), h.z);
}
float sdCylinder(vec3 p, vec3 a, vec3 axis, float r, float len) {
  vec3 d = p - a;
  float t = dot(d, axis);
  float radial = length(d - axis * t);
  vec2 w = vec2(radial - r, abs(t - 0.5 * len) - 0.5 * len);
  return min(max(w.x, w.y), 0.0) + length(max(w, 0.0));
}
float sdCone(vec3 p, vec3 a, vec3 b, float ra, float rb) {
  float rba = rb - ra;
  float baba = dot(b - a, b - a);
  float papa = dot(p - a, p - a);
  float paba = dot(p - a, b - a) / baba;
  float x = sqrt(max(0.0, papa - paba * paba * baba));
  float cax = max(0.0, x - ((paba < 0.5) ? ra : rb));
  float cay = abs(paba - 0.5) - 0.5;
  float k = rba * rba + baba;
  float f = clamp((rba * (x - ra) + paba * baba) / k, 0.0, 1.0);
  float cbx = x - ra - f * rba;
  float cby = paba - f;
  float s = (cbx < 0.0 && cay < 0.0) ? -1.0 : 1.0;
  return s * sqrt(min(cax * cax + cay * cay * baba, cbx * cbx + cby * cby * baba));
}
float sdTriangle2(vec2 p, vec2 p1, vec2 p2) {
  vec2 p0 = vec2(0.0);
  vec2 e0 = p1 - p0, e1 = p2 - p1, e2 = p0 - p2;
  vec2 v0 = p - p0, v1 = p - p1, v2 = p - p2;
  vec2 pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
  vec2 pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
  vec2 pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
  float s = sign(e0.x * e2.y - e0.y * e2.x);
  vec2 d = min(min(vec2(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x)),
                   vec2(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x))),
                   vec2(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x)));
  return -sqrt(d.x) * sign(d.y);
}
float sdRib(vec3 p, vec3 o, vec3 u, vec3 v, vec3 n, float p1x, vec2 p2, float ht) {
  vec3 d = p - o;
  return extrudeField(sdTriangle2(vec2(dot(d, u), dot(d, v)), vec2(p1x, 0.0), p2), dot(d, n), ht);
}
float sdSlot(vec3 p, vec3 c, vec3 u, vec3 v, vec3 n, float hl, float r, float depth) {
  vec3 d = p - c;
  float x = max(abs(dot(d, u)) - hl, 0.0);
  float d2 = length(vec2(x, dot(d, v))) - r;
  return extrudeField(d2, dot(d, n) - 0.5 * depth, 0.5 * depth);
}
float opUnion(float a, float b) { return min(a, b); }
float opUnionRound(float a, float b, float r) {
  vec2 u = max(vec2(r - a, r - b), vec2(0.0));
  return max(r, min(a, b)) - length(u);
}
float opSubtract(float a, float b) { return max(a, -b); }
float opIntersect(float a, float b) { return max(a, b); }
`

export const CONCEPT_VERTEX_SHADER = `
varying vec2 vUv;
void main() {
  vUv = uv;
  gl_Position = vec4(position.xy, 0.0, 1.0);
}
`

/** POV codes shared by the shader and the card. */
export const POV_CODE = { sketch: 0, load: 1, mill: 2, print: 3 } as const
export type PovCode = (typeof POV_CODE)[keyof typeof POV_CODE]

/** The full fragment shader for one recipe. */
export function conceptFragmentShader(recipe: Recipe): string {
  const N = MAX_LOAD_POINTS
  return `
precision highp float;
varying vec2 vUv;
uniform vec2 uRes;
uniform vec3 uTarget;
uniform vec3 uRight;
uniform vec3 uUp;
uniform vec3 uFwd;
uniform float uHalf;
uniform vec3 uBMin;
uniform vec3 uBMax;
uniform int uPov;
uniform vec3 uInk;
uniform vec3 uFace;
uniform vec3 uWarn;
uniform float uReveal;
uniform vec3 uLoad[${N}];
uniform int uNLoad;
uniform vec3 uSup[${N}];
uniform int uNSup;
uniform float uSigma;
uniform float uZmin;
uniform float uCell;
${GLSL_SDF_LIBRARY}
${sceneGlsl(recipe)}

vec3 sceneNormal(vec3 p, float e) {
  vec2 k = vec2(1.0, -1.0);
  return normalize(k.xyy * sceneSdf(p + k.xyy * e) + k.yyx * sceneSdf(p + k.yyx * e) +
                   k.yxy * sceneSdf(p + k.yxy * e) + k.xxx * sceneSdf(p + k.xxx * e));
}

float segDist(vec3 p, vec3 a, vec3 b) {
  vec3 ab = b - a;
  float l2 = dot(ab, ab);
  float t = l2 > 0.0 ? clamp(dot(p - a, ab) / l2, 0.0, 1.0) : 0.0;
  return length(p - (a + ab * t));
}

// concept-pov.ts loadPathWeight
float loadWeight(vec3 p) {
  float best = 1e20;
  for (int i = 0; i < ${N}; i++) {
    if (i >= uNLoad) break;
    for (int j = 0; j < ${N}; j++) {
      if (j >= uNSup) break;
      best = min(best, segDist(p, uLoad[i], uSup[j]));
    }
  }
  if (best > 1e19 || uSigma <= 0.0) return 0.0;
  float r = best / uSigma;
  return exp(-r * r);
}

// concept-pov.ts reachableAlongZ
bool reachZ(vec3 p, vec3 n, float dir, float diag) {
  float off = max(1e-3 * diag, 1e-4);
  float hit = 0.5 * off;
  vec3 q = p + n * off;
  float zEnd = dir > 0.0 ? uBMax.z + off : uBMin.z - off;
  float span = abs(zEnd - q.z);
  float t = 0.0;
  for (int i = 0; i < 160; i++) {
    if (t > span) return true;
    float d = sceneSdf(q + vec3(0.0, 0.0, dir * t));
    if (d < hit) return false;
    t += max(d, hit);
  }
  return true;
}

float hatch(float spacing, float slope) {
  float c = gl_FragCoord.x * slope + gl_FragCoord.y;
  return step(mod(c, spacing), 1.0);
}

void main() {
  // Top-to-bottom print: rows below the reveal line are not drawn yet.
  if (1.0 - gl_FragCoord.y / uRes.y > uReveal) discard;
  vec2 ndc = vUv * 2.0 - 1.0;
  float aspect = uRes.x / uRes.y;
  float diag = length(uBMax - uBMin);
  vec3 ro = uTarget + uRight * ndc.x * uHalf * aspect + uUp * ndc.y * uHalf - uFwd * diag * 2.0;
  vec3 rd = uFwd;
  // Clip the ray to the recipe's bounds.
  vec3 pad = vec3(0.02 * diag);
  vec3 rdS = vec3(abs(rd.x) < 1e-6 ? 1e-6 : rd.x, abs(rd.y) < 1e-6 ? 1e-6 : rd.y, abs(rd.z) < 1e-6 ? 1e-6 : rd.z);
  vec3 inv = 1.0 / rdS;
  vec3 t0s = (uBMin - pad - ro) * inv;
  vec3 t1s = (uBMax + pad - ro) * inv;
  vec3 tmin = min(t0s, t1s);
  vec3 tmax = max(t0s, t1s);
  float tn = max(max(tmin.x, tmin.y), tmin.z);
  float tf = min(min(tmax.x, tmax.y), tmax.z);
  float pix = 2.0 * uHalf / uRes.y;
  if (tn > tf || tf < 0.0) discard;
  float t = max(tn, 0.0);
  float eps = 0.25 * pix;
  float minD = 1e20;
  bool hit = false;
  for (int i = 0; i < 200; i++) {
    float d = sceneSdf(ro + rd * t);
    minD = min(minD, d);
    if (d < eps) { hit = true; break; }
    t += d;
    if (t > tf) break;
  }
  if (!hit) {
    // The silhouette: a ray that passed within a pixel of the surface.
    if (minD < 1.1 * pix) gl_FragColor = vec4(uInk, 0.9);
    else discard;
    return;
  }
  vec3 p = ro + rd * t;
  vec3 n = sceneNormal(p, 0.5 * pix);
  vec3 nWide = sceneNormal(p, 3.0 * pix);
  float crease = 1.0 - dot(n, nWide);
  vec3 light = normalize(-uFwd + 0.7 * uUp - 0.45 * uRight);
  float lam = max(dot(n, light), 0.0);
  float tone = lam > 0.62 ? 0.0 : lam > 0.3 ? 1.0 : 2.0;
  float ink = 0.0;
  if (tone >= 1.0) ink = max(ink, 0.55 * hatch(5.0, 1.0));
  if (tone >= 2.0) ink = max(ink, 0.55 * hatch(5.0, -1.0));
  vec3 col = uFace;
  float warn = 0.0;
  float alpha = 1.0;
  if (uPov == ${POV_CODE.load}) {
    float w = smoothstep(${glslFloat(LOAD_PATH_ON - 0.15)}, ${glslFloat(LOAD_PATH_ON + 0.15)}, loadWeight(p));
    alpha = mix(0.3, 1.0, w);
    ink *= mix(0.25, 1.0, w);
  } else if (uPov == ${POV_CODE.mill}) {
    bool stock = n.z < 0.0 && p.z - uZmin < 0.5 * uCell;
    if (!stock && !reachZ(p, n, 1.0, diag)) {
      if (reachZ(p, n, -1.0, diag)) {
        ink = max(ink, 0.8 * hatch(3.0, 0.0));
      } else {
        col = mix(uFace, uWarn, 0.35);
        warn = 0.9 * max(hatch(4.0, 1.0), hatch(4.0, -1.0));
      }
    }
  } else if (uPov == ${POV_CODE.print}) {
    bool bed = n.z < 0.0 && p.z - uZmin < 0.5 * uCell;
    if (!bed && -n.z > ${glslFloat(OVERHANG_COS)} + 1e-9) {
      col = mix(uFace, uWarn, 0.35);
      warn = 0.9 * hatch(4.0, 1.0);
    }
  }
  float edge = smoothstep(0.03, 0.08, crease);
  col = mix(col, uWarn, warn);
  col = mix(col, uInk, max(ink, edge));
  gl_FragColor = vec4(col, alpha);
}
`
}
