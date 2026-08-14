/**
 * The task spec.
 *
 * Two fields are mandatory and both are load-bearing:
 *
 *   `toolAllowlist` — the action space, FROZEN per task. `find_tool` ranks by
 *   IDF over the tool corpus, so any new tool perturbs rankings corpus-wide.
 *   That is correct for a product and wrong for an environment: a drifting
 *   action space makes two trajectories incomparable. The metatools
 *   (find_tool, describe_tool, workbench) appear only when a task names them
 *   — a task ABOUT tool discovery is legitimate, but it is a different task.
 *
 *   `claims` — machine-checkable ground truth, closed by `verify_claim`.
 *   Without them scoring falls back to a model's opinion, which is the exact
 *   thing this environment exists to avoid.
 *
 * ─── A CLAIM IS WRITTEN IN verify_claim's LANGUAGE, NOT IN PROSE ─────────
 *
 * `verify_claim`'s schema (roshera-mcp/src/tools/inspect.ts:63-99) is
 * `{expr, bindings:[{var, measure}], expected, tolerance?}` over a CLOSED set
 * of five measure kinds — volume, surface_area, face_area, edge_length,
 * constant (inspect.ts:73-89; the same closed enum on the kernel side is
 * `Measurement`, geometry-engine/src/readable/claim.rs:26-40). A claim
 * written as `{quantity: "radius"}` is not a misspelling of that language: it
 * is not expressible in it at all, and a task whose ground truth cannot be
 * checked has no ground truth. So the shape is validated HERE, at task
 * definition, where the failure is loud, rather than discovered at scoring
 * time when the episode is already over.
 *
 * `measure.part` names a solid by object UUID, or by the recipe-local token
 * `solid:N` meaning "the Nth object_uuid this episode created" — the same
 * symbolic-operand convention `recipe_get` uses for re-issued builds
 * (tools/timeline.ts:220-222); `mcp_session.claims()` resolves it.
 *
 * Every claim carries a tolerance. Geometry reproduces to ~4e-8, so an exact
 * float equality claim can never pass; a claim that cannot pass is a broken
 * task, not a hard one.
 */

import { digestOf } from "./provenance.mjs";

const SPLITS = Object.freeze(["train", "eval"]);

/** The closed measure enum, verbatim from tools/inspect.ts:73-80. */
const MEASURE_KINDS = Object.freeze([
  "volume",
  "surface_area",
  "face_area",
  "edge_length",
  "constant",
]);

/**
 * Validate one binding against the real schema. Each kind names a DIFFERENT
 * companion field, and the backend deserialises the tagged union strictly
 * (api-server/src/handlers/agent.rs:144-165), so a `face_area` binding
 * carrying a `part` is rejected on the wire rather than measured.
 */
function checkBinding(taskId, claimName, b) {
  const where = `task ${taskId}: claim ${claimName}`;
  if (typeof b?.var !== "string" || b.var.trim() === "") {
    throw new Error(`${where}: every binding needs a non-empty \`var\` naming a variable in \`expr\``);
  }
  const m = b.measure;
  if (!MEASURE_KINDS.includes(m?.kind)) {
    throw new Error(
      `${where}: binding '${b.var}' has measure kind ${JSON.stringify(m?.kind)}, which ` +
      `verify_claim cannot measure. The kinds are a CLOSED set — ` +
      `${MEASURE_KINDS.join(", ")} (tools/inspect.ts:73-80) — so a quantity ` +
      `outside it (a radius, a height) is not expressible and the claim could ` +
      `never be checked`,
    );
  }
  if (m.kind === "volume" || m.kind === "surface_area") {
    if (typeof m.part !== "string" || m.part.trim() === "") {
      throw new Error(
        `${where}: a ${m.kind} binding needs \`part\` — an object UUID or the ` +
        `token 'solid:N' naming the Nth solid the episode created`,
      );
    }
  } else if (m.kind === "face_area") {
    if (!Number.isInteger(m.face)) throw new Error(`${where}: a face_area binding needs an integer \`face\` id`);
  } else if (m.kind === "edge_length") {
    if (!Number.isInteger(m.edge)) throw new Error(`${where}: an edge_length binding needs an integer \`edge\` id`);
  } else if (!Number.isFinite(m.value)) {
    throw new Error(`${where}: a constant binding needs a finite \`value\``);
  }
}

export function defineTask({
  id, family, prompt, toolAllowlist, claims, stepBudget, tokenBudget, split,
}) {
  if (typeof id !== "string" || id.trim() === "") {
    throw new Error("a task needs a non-empty id");
  }
  const resolvedFamily = family === undefined ? id : family;
  if (typeof resolvedFamily !== "string" || resolvedFamily.trim() === "") {
    throw new Error(`task ${id}: family must be a non-empty string when given`);
  }
  if (typeof prompt !== "string" || prompt.trim() === "") {
    throw new Error(`task ${id}: a task needs a prompt`);
  }
  if (!Array.isArray(toolAllowlist) || toolAllowlist.length === 0) {
    throw new Error(
      `task ${id}: an empty tool allowlist is not an action space. The ` +
      `allowlist is frozen per task because find_tool's IDF ranking shifts ` +
      `corpus-wide when tools change, and a moving action space makes two ` +
      `trajectories incomparable`,
    );
  }
  if (!Array.isArray(claims) || claims.length === 0) {
    throw new Error(
      `task ${id}: a task with no claim has no ground truth, and scoring ` +
      `would fall back to a model's opinion`,
    );
  }
  for (const c of claims) {
    if (typeof c?.name !== "string" || c.name.trim() === "") {
      throw new Error(`task ${id}: every claim needs a non-empty name`);
    }
    if (typeof c.expr !== "string" || c.expr.trim() === "") {
      throw new Error(
        `task ${id}: claim ${c.name} needs an \`expr\` — verify_claim evaluates a ` +
        `math expression over its bindings (tools/inspect.ts:64-66); there is no ` +
        `other way to state a checkable quantity`,
      );
    }
    if (!Array.isArray(c.bindings) || c.bindings.length === 0) {
      throw new Error(
        `task ${id}: claim ${c.name} needs at least one binding — an expression ` +
        `with no variable bound to a kernel measurement is not checked against ` +
        `geometry at all`,
      );
    }
    for (const b of c.bindings) checkBinding(id, c.name, b);
    if (typeof c.expected !== "number" || !Number.isFinite(c.expected)) {
      throw new Error(`task ${id}: claim ${c.name} needs a finite \`expected\` value`);
    }
    if (typeof c.tolerance !== "number" || !Number.isFinite(c.tolerance) || !(c.tolerance > 0)) {
      throw new Error(
        `task ${id}: claim ${c.name} needs a finite, positive tolerance — ` +
        `geometry reproduces to ~4e-8, so an exact equality claim can never ` +
        `pass, and an infinite tolerance makes the claim unfalsifiable: it ` +
        `can never fail at scoring time regardless of what the kernel ` +
        `actually measured`,
      );
    }
  }
  if (!SPLITS.includes(split)) {
    throw new Error(`task ${id}: split must be one of ${SPLITS.join(", ")}`);
  }
  if (!Number.isInteger(stepBudget) || stepBudget <= 0) {
    throw new Error(`task ${id}: stepBudget must be a positive integer`);
  }
  if (!Number.isInteger(tokenBudget) || tokenBudget <= 0) {
    throw new Error(`task ${id}: tokenBudget must be a positive integer`);
  }
  return Object.freeze({
    id, family: resolvedFamily, prompt,
    toolAllowlist: Object.freeze([...toolAllowlist]),
    claims: Object.freeze(claims.map((c) => Object.freeze({
      ...c,
      bindings: Object.freeze(c.bindings.map((b) => Object.freeze({
        ...b, measure: Object.freeze({ ...b.measure }),
      }))),
    }))),
    stepBudget, tokenBudget, split,
  });
}

const R = 25;
const H = 60;

/**
 * The seed set. Deliberately ONE task for slice 1: this slice proves the
 * episode loop, not task coverage. Volume arrives in slice 3, where a
 * parametric family derives its claims from the sampled parameters — for a
 * parametric family the REQUEST is the ground truth.
 *
 * WHY VOLUME AND SURFACE AREA rather than "radius" and "height": those two are
 * what `verify_claim` can actually measure (the closed enum above). Checking a
 * made-up `quantity: "radius"` would have checked nothing at all.
 *
 * ─── WHAT THIS PAIR DOES AND DOES NOT CONSTRAIN — A KNOWN BLIND SPOT ─────
 *
 * It does NOT pin (r, h). An earlier version of this comment claimed it did,
 * on the grounds that πr²h and 2πr(r+h) are "independent". They are not
 * independent enough: at FIXED volume, A(r) = 2πr² + 2V/r has a minimum at
 * r = (V/2π)^(1/3) ≈ 26.5665 mm for this task's V, so every radius on the
 * falling branch has a CONJUGATE on the rising branch with the same V and the
 * same A. The requested r = 25 sits on the falling branch, and the conjugate
 * is
 *
 *     r = 28.197051490249287 mm,  h = 47.165438120937125 mm
 *
 * which reproduces this task's volume EXACTLY (ΔV = 0) and its surface area to
 * 3.6e-12 mm² — both far inside the 1e-3 relative bands below. A slice-2 model
 * policy that built that cylinder would have BOTH claims verified true for a
 * dimensionally wrong part. `test/task.test.mjs` pins this conjugate so the
 * false theorem cannot quietly return.
 *
 * What the pair DOES constrain, and it is not nothing: any error in r alone,
 * or in h alone, moves V (monotone in each with the other fixed) and fails —
 * r = 25.1 moves the volume 0.8%, eight times the band. The blind spot is
 * exactly the two-parameter conjugate above, not sloppiness in general.
 *
 * WHY IT IS NOT FIXED HERE. A cap's `face_area` is πr², which would pin r on
 * its own, and V would then pin h — the fix is obvious and it is NOT
 * EXPRESSIBLE in this language today. `face_area` binds a RAW model-global
 * face id: `Measurement::FaceArea { face: u32 }`
 * (geometry-engine/src/readable/claim.rs:34), resolved by
 * `model.query_face(*face)` (claim.rs:86) with no part scoping and no
 * "the cap of solid:0" selector; the MCP schema is likewise a bare
 * `face: z.number().int()` (roshera-mcp/src/tools/inspect.ts:86), and
 * `edge_length` — a cap circle's 2πr would pin r too — has exactly the same
 * shape (claim.rs:36, inspect.ts:87). A task's claims are frozen HERE, at
 * module load, while that id is minted by the kernel mid-episode, and
 * `verifyClaims` resolves only the `solid:N` token, passing `face` / `edge`
 * verbatim (lib/mcp_session.mjs:284-287). Hardcoding an id would be inventing
 * a contract, which is the one thing this package refuses to do.
 *
 * And no additional claim over {volume, surface_area, constant} can close it:
 * the conjugate has identical V and A, so every expression over those inputs
 * returns the identical value for both cylinders. A third INDEPENDENT
 * measurement is required, not a cleverer expression.
 *
 * SLICE 3 POINTER: give a binding a symbolic face/edge token resolved by the
 * session the way `solid:N` already is (mcp_session.mjs `resolveSolidRef`) —
 * the session can discover a cap from the built part and bind its real id —
 * or, for a parametric family, derive the claims from the sampled parameters,
 * where the REQUEST is the ground truth and the fidelity block already
 * compares requested against measured.
 *
 * TOLERANCE — 1e-3 relative, a judgement stated rather than hidden. The
 * kernel measures these exactly (mass properties come from a divergence-
 * theorem integral over the analytic surfaces, not from tessellation —
 * tools/inspect.ts:38-41 "Exact mass properties"), so the true error should
 * be far below this; 1e-3 leaves room for that claim to be slightly wrong
 * without the task becoming unpassable, while still failing loudly on a real
 * dimensional error (r=25.1 moves the volume by 0.8%, eight times this band).
 */
export const TASKS = Object.freeze([
  defineTask({
    id: "cylinder-r25-h60",
    prompt:
      "Create a single cylinder with radius 25 mm and height 60 mm, then " +
      "verify it. Declare a design intent first.",
    toolAllowlist: ["timeline_checkpoint", "create_cylinder", "verify_part"],
    claims: [
      {
        name: "volume",
        expr: "v",
        bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
        expected: Math.PI * R * R * H,
        tolerance: Math.PI * R * R * H * 1e-3,
      },
      {
        name: "surface_area",
        expr: "a",
        bindings: [{ var: "a", measure: { kind: "surface_area", part: "solid:0" } }],
        expected: 2 * Math.PI * R * (R + H),
        tolerance: 2 * Math.PI * R * (R + H) * 1e-3,
      },
    ],
    stepBudget: 12,
    tokenBudget: 40000,
    split: "train",
  }),
]);

export function taskById(id) {
  return TASKS.find((t) => t.id === id);
}

/** Re-exported for callers that only want the digest, not the assembler. */
export const taskDigest = digestOf;

