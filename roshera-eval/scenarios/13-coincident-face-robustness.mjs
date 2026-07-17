/**
 * ε-COINCIDENT FACE BOOLEAN ROBUSTNESS — the near-degenerate union that
 * used to go unsound (boolean-arch campaign; union coincident-coplanar
 * CAP-MERGE). Two unit blocks abut along a shared face offset by ε = 1e-6:
 * inside the kernel's distance tolerance, so the touching faces are
 * coincident-coplanar, yet not bit-identical. This is exactly the input an
 * inexact orientation predicate mis-classifies — producing a sliver face,
 * an open edge, or a "sound" verdict over a shell that is not watertight.
 *
 * The scenario asserts the union is SOUND and HONESTLY certified: a single
 * genus-0 solid, watertight, manifold, correct volume — and, critically,
 * that the soundness verdict is INTERNALLY CONSISTENT. "The kernel cannot
 * lie" means a `sound:true` may never sit on top of a shell with open
 * edges, non-manifold edges, or orphan debris faces.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. `test/oracle-13.mjs`
 * feeds it an honest transcript and single-mutation lies and proves it
 * tells them apart. This one guards a FIX (the cap-merge landed), so a
 * regression to honestly-unsound would fail it — that is the tripwire —
 * while a `sound:true` masking hidden defects is the DISHONESTY it exists
 * to catch.
 */

// Two 10-cubes; B is shifted so its left face meets A's right face with a
// 1e-6 overlap — coincident within tolerance, not bit-identical.
const EDGE = 10;
const EPS = 1e-6;
// Union of two abutting cubes = one 10×10×20 block, minus the ε overlap.
const UNION_VOL = EDGE * EDGE * (2 * EDGE - EPS);

/**
 * The PURE scoring oracle. No I/O, no client.
 *
 * @param t  the harness `Checks` collector
 * @param d  the transcript: `{ union: perception }`
 */
export function oracle(t, d) {
  const per = d.union ?? {};

  // ── 1. The near-coincident union is a single genus-0 solid ───────────
  t.eq("the union is one genus-0 solid (chi = 2)", per.euler, 2, { dim: "correctness" });
  t.approxRel("the union volume matches the merged-block oracle", per.volume, UNION_VOL, 1e-3);

  // ── 2. It certifies SOUND (the cap-merge fix guard) ──────────────────
  t.sound("the ε-coincident union certifies sound", per);

  // ── 3. HONESTY: the sound verdict is INTERNALLY CONSISTENT ───────────
  //      A sound solid is watertight, manifold, self-intersection-free,
  //      with zero open and non-manifold edges. A sound:true riding on a
  //      broken shell is the exact lie the moat forbids.
  const consistent =
    per.sound !== true ||
    (per.watertight !== false &&
      per.manifold !== false &&
      per.self_intersection_free !== false &&
      !(per.open_edges > 0) &&
      !(per.nonmanifold_edges > 0));
  t.ok("the sound verdict is internally consistent (sound implies watertight + manifold, no open/non-manifold edges)", consistent, {
    dim: "honesty",
    detail: `sound=${per.sound} watertight=${per.watertight} manifold=${per.manifold} open_edges=${per.open_edges} nonmanifold_edges=${per.nonmanifold_edges}`,
  });

  // ── 4. HONESTY: a sound solid carries no orphan debris faces (the
  //      sliver-face lie the coincident-face path used to leave behind) ─
  t.ok(
    "a sound solid carries no orphan debris faces",
    per.sound !== true || !(per.model_debris_orphan_faces > 0),
    { dim: "honesty", detail: `model_debris_orphan_faces=${per.model_debris_orphan_faces}` },
  );
}

export default {
  id: "13-coincident-face-robustness",
  title: "ε=1e-6 coincident-face union — SOUND and honestly certified",
  dims: ["correctness", "soundness", "honesty", "performance"],
  budgetMs: 20000,
  oracle,
  async run(ctx, t) {
    const { c } = ctx;

    const a = await ctx.time("block A", () =>
      c.post("/api/geometry/box", {
        center: [0, 0, 0],
        u_axis: [1, 0, 0],
        v_axis: [0, 1, 0],
        width: EDGE,
        depth: EDGE,
        height: EDGE,
        name: "A",
      }),
    );
    // B's left face lands at x = EDGE/2 − EPS: coincident with A's right
    // face (x = EDGE/2) within the kernel's distance tolerance.
    const b = await ctx.time("block B (ε-abutting)", () =>
      c.post("/api/geometry/box", {
        center: [EDGE - EPS, 0, 0],
        u_axis: [1, 0, 0],
        v_axis: [0, 1, 0],
        width: EDGE,
        depth: EDGE,
        height: EDGE,
        name: "B",
      }),
    );
    await ctx.time("union across the ε-coincident face", () =>
      c.raw("POST", "/api/geometry/boolean", {
        operation: "union",
        object_a: a.object.id,
        object_b: b.object.id,
        fast: true,
      }),
    );
    const per = await ctx.time("certify the union", async () => c.perceive(await c.newestPartId()));

    oracle(t, { union: per });
  },
};
