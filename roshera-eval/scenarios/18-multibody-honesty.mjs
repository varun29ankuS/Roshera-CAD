/**
 * MULTI-BODY HONESTY — two kernel defects found 2026-08-08 by hand-probing
 * that this suite should have caught and recorded. Both are scored against
 * INDEPENDENT closed-form oracles computed here, never against a value the
 * kernel itself produced — the same discipline as 12/14's honesty oracles.
 *
 *   A. UNION OF TWO DISJOINT SOLIDS DROPS TOPOLOGY, and still certifies the
 *      result SOUND. Two 10mm cubes 30mm apart, unioned: closed-form volume
 *      = 1000 + 1000 = 2000.0, face count = 6 + 6 = 12 (the result is a
 *      compound of two separate hexahedra — nothing overlaps, nothing
 *      should be dropped). MEASURED against the live REST API
 *      (2026-08-08): `volume` comes back correct (2000.0, via mesh
 *      integration over BOTH bodies' tessellation) but the B-Rep topology
 *      report (`GET /api/agent/parts/:id`, `topology.face_count`) says 6
 *      faces, 12 edges, 8 vertices — HALF a compound, one whole body's
 *      B-Rep shell missing from the topology summary — while `sound: true`
 *      is still asserted. A correct mesh riding on top of a wrong topology
 *      report, both under a clean certificate, is exactly the kind of
 *      self-contradiction "the kernel cannot lie" forbids.
 *
 *   B. THE SECOND THROUGH-BORE IN A BOX MAKES IT UNSOUND — pinned as a RED
 *      kernel-level test in
 *      `roshera-backend/geometry-engine/tests/boolean_multibody.rs`
 *      (`second_through_bore_stays_sound_and_volume_matches_analytic`),
 *      which calls `boolean_operation` directly against a `BRepModel` and
 *      gets a dropped face / open boundary edges / a volume that RISES on
 *      the second subtraction. MEASURED HERE (2026-08-08) at the REST
 *      layer, with the geometrically-equivalent construction (80x40x40mm
 *      plate, two independent r=1 through-bores along Z, one at x=-30 and
 *      one at x=0, 28mm of solid material between them, `fast: true` and
 *      `fast` omitted both tried): the live api-server currently returns a
 *      CORRECT, SOUND result — volume 127748.71 vs the closed-form
 *      127748.67, 0 open edges. The REST /api/geometry/boolean path does
 *      NOT reproduce defect B for this construction today, even though the
 *      kernel-level unit test does. Rather than paper over that gap, part B
 *      below is kept as a REGRESSION GUARD (it is expected to stay GREEN)
 *      and the docblock states the discrepancy plainly: either the
 *      api-server takes a different code path / BooleanOptions than the
 *      kernel test's `BooleanOptions::default()`, or the trigger needs a
 *      more precise construction than this run found. Only the closed-form
 *      127748.67 is asserted, never a recorded "buggy" number.
 *
 * # KNOWN-RED
 *
 * `knownRed: true` below reflects part A, which DOES fail at the REST
 * layer today (the face-count/topology honesty check) — that is this
 * scenario's live tripwire. Part B's checks are expected to PASS (a
 * regression guard, not a currently-red assertion); if they ever start
 * failing, that means defect B has surfaced through the REST path too.
 * When part A is fixed, delete `knownRed` so the suite stops tolerating it.
 *
 * No `expectFail` / `known-red` / quarantine convention existed anywhere in
 * roshera-eval before this file (grepped lib/ and scenarios/ — nothing).
 * Rather than silently bump the suite's failed-scenario exit code (which
 * would make `node run.mjs` look broken forever and train everyone to
 * ignore a nonzero exit), this introduces the minimal convention: a
 * scenario may set `knownRed: true`. The harness (lib/harness.mjs) still
 * scores it PASS/FAIL honestly and prints every failing check — nothing is
 * hidden — but run.mjs excludes `knownRed` scenarios from the process exit
 * code, and the scorecard marks them `FAIL(kr)` instead of `FAIL ✗` so a
 * human glancing at the table can tell "expected failure" from "regression".
 * If a `knownRed` scenario ever PASSES, run.mjs prints an explicit
 * "regression fixed?" note, because that is exactly the signal that a
 * defect has been fixed and the flag should come off.
 */

// ── A. Disjoint union — closed form ─────────────────────────────────────
const CUBE = 10; // mm, each cube is CUBE^3
const UNION_VOL = CUBE ** 3 + CUBE ** 3; // 2000.0 — both bodies retained
const UNION_FACES = 6 + 6; // 12 — two independent hexahedral shells

// ── B. Two non-intersecting through-bores — closed form ────────────────
const PLATE = { w: 80, d: 40, h: 40 }; // width(X) x depth(Y) x height(Z)
const BORE_R = 1;
const BORE_THROUGH_LEN = PLATE.h; // bores travel the Z (height) dimension
const PLATE_VOL = PLATE.w * PLATE.d * PLATE.h; // 128000
const BORE_VOL =
  PLATE_VOL - 2 * (Math.PI * BORE_R * BORE_R * BORE_THROUGH_LEN); // 127748.67...

export default {
  id: "18-multibody-honesty",
  title: "Multi-body honesty — disjoint-union body-drop + double-bore unsoundness (known-red)",
  dims: ["correctness", "soundness", "honesty"],
  budgetMs: 20000,
  knownRed: true, // see docblock — delete once A and B are both fixed
  async run(ctx, t) {
    const { c } = ctx;

    // ═══ A. Union of two solids 30mm apart must retain BOTH bodies ══════
    const boxA = await ctx.time("cube A 10^3 at origin", () =>
      c.post("/api/geometry/box", {
        center: [0, 0, 0],
        u_axis: [1, 0, 0],
        v_axis: [0, 1, 0],
        width: CUBE,
        depth: CUBE,
        height: CUBE,
        name: "cubeA",
      }),
    );
    // Gap between the facing faces is exactly 30mm: cubeA spans x[-5,5],
    // cubeB centered at x=40 spans x[35,45] -> gap = 35 - 5 = 30.
    const boxB = await c.post("/api/geometry/box", {
      center: [40, 0, 0],
      u_axis: [1, 0, 0],
      v_axis: [0, 1, 0],
      width: CUBE,
      depth: CUBE,
      height: CUBE,
      name: "cubeB",
    });
    const unionOp = await ctx.time("union of disjoint 10mm cubes (30mm gap)", () =>
      c.raw("POST", "/api/geometry/boolean", {
        operation: "union",
        object_a: boxA.object.id,
        object_b: boxB.object.id,
        fast: true,
      }),
    );
    t.ok("disjoint union completes (200) rather than crashing", unionOp.ok, {
      dim: "soundness",
      detail: `status=${unionOp.status}`,
    });
    let perUnion = null;
    if (unionOp.ok) {
      perUnion = await c.perceive(await c.newestPartId());
    }

    t.approxRel(
      "disjoint union volume matches closed form (both bodies retained: 1000+1000)",
      perUnion?.volume,
      UNION_VOL,
      1e-9,
    );
    t.eq(
      "disjoint union face count matches closed form (both bodies retained: 6+6)",
      perUnion?.face_count,
      UNION_FACES,
    );
    // The core honesty violation, as MEASURED (2026-08-08): the mesh-integrated
    // volume comes back correct (2000.0, both bodies tessellated), but the
    // B-Rep topology report says 6 faces — half a compound — while `sound`
    // is still `true`. A clean certificate over an internally-contradictory
    // report (right volume, wrong topology) is the kernel vouching for a
    // number it did not actually verify end-to-end.
    t.ok(
      "a SOUND disjoint union must not report a topology missing an entire body's faces",
      !(perUnion?.sound === true && perUnion?.face_count !== UNION_FACES),
      {
        dim: "honesty",
        detail: `sound=${perUnion?.sound} volume=${perUnion?.volume} face_count=${perUnion?.face_count} (expected volume=${UNION_VOL}, faces=${UNION_FACES})`,
      },
    );

    // ═══ B. A second, non-intersecting through-bore must stay sound ════
    const plate = await ctx.time("plate 80x40x40", () =>
      c.post("/api/geometry/box", {
        center: [0, 0, 0],
        u_axis: [1, 0, 0],
        v_axis: [0, 1, 0],
        width: PLATE.w,
        depth: PLATE.d,
        height: PLATE.h,
        name: "plate",
      }),
    );
    let uuid = plate.object.id;

    const boreLeft = await c.post("/api/geometry/cylinder", {
      center: [-30, 0, 0],
      axis: [0, 0, 1],
      radius: BORE_R,
      height: 60, // over-long, guarantees full penetration through the 40mm slab
      name: "boreLeft",
    });
    await c.post("/api/geometry/boolean", {
      operation: "difference",
      object_a: uuid,
      object_b: boreLeft.object.id,
      fast: true,
    });
    uuid = await c.uuidForPart(await c.newestPartId());
    const perSingleBore = await c.perceive(await c.newestPartId());
    t.sound("single through-bore (x=-30) alone is sound", perSingleBore, { dim: "soundness" });

    const boreCenter = await c.post("/api/geometry/cylinder", {
      center: [0, 0, 0],
      axis: [0, 0, 1],
      radius: BORE_R,
      height: 60,
      name: "boreCenter",
    });
    const boreOp = await ctx.time("second, non-intersecting through-bore (x=0)", () =>
      c.raw("POST", "/api/geometry/boolean", {
        operation: "difference",
        object_a: uuid,
        object_b: boreCenter.object.id,
        fast: true,
      }),
    );
    t.ok("second through-bore completes (200) rather than crashing", boreOp.ok, {
      dim: "soundness",
      detail: `status=${boreOp.status}`,
    });
    let perTwoBore = null;
    if (boreOp.ok) {
      perTwoBore = await c.perceive(await c.newestPartId());
    }

    t.sound(
      "plate with two non-intersecting through-bores is sound (unrelated bore must not break the first)",
      perTwoBore,
    );
    t.approxRel(
      "two-bore plate volume matches closed form (128000 - 2*pi*r^2*40)",
      perTwoBore?.volume,
      BORE_VOL,
      1e-6,
    );
  },
};
