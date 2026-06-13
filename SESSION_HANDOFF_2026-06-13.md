# Session Handoff — 2026-06-13

Marathon session (continued from 2026-06-12 overnight). Human-readable
router; the durable detail lives in the auto-memory topic files named
below (under `~/.claude/projects/.../memory/`). Read those FIRST on
resume.

---

## ▶ ACTIVE WORK — START HERE

**Boolean #27 chained-union B-Rep bug — PARTS 1 & 2 SHIPPED & GREEN;
PART 3 (final) localized.** It's a 3-defect campaign, not the "2/2" the
earlier note predicted. Read `boolean-coaxial-cap-bug.md` "PART 2
SHIPPED" + "PART 3 PRECISELY LOCALIZED" sections first.

- **PART 1 GREEN (f35b74e):** merge nest-guard — never nest a fragment
  into an existing hole.
- **PART 2 GREEN (c7ba18a, this session):** upstream void-cut filter in
  `split_face_by_curves` (~5477, before the cut-add loop) — for planar
  faces with pre-existing holes, skip any cut whose samples all project
  strictly inside a hole polygon. Effect on prism_chain_diag_27: euler
  32→**2**, nonmanifold 61→**32**, vol +7.6%→~exact. Floor GREEN: boolean
  lib 89/0, curved poke 4/0, cargo check clean. NOT yet a valid solid.
- **PART 3 (the actual finish line) — LOCALIZED, NOT a quick patch.**
  Residual: 96 open + 64 triple-used B-Rep edges (two rings: r35 192-223,
  r60 288-319). Root: the merge/partition layer NESTS AN ADJACENT COPLANAR
  SIBLING AS A HOLE on the composite z=0 base. `face_idx=64 orig=102`
  (r60..80 annulus) carries the r60 ring in BOTH outer boundary AND
  inner_loops (malformed); `orig=103` is double-kept as annulus r35..60
  AND flat r35 disk. Fix: (a) never attach a cycle as an inner loop when
  it's also part of the face's outer boundary (shared ring = adjacency,
  not containment); (b) dedupe the orig=103 double-keep. FIRST trace the
  attach site (merge_same_origin_fragments ~9991 vs
  partition_outer_and_pre_existing_hole_cycles ~5872) before editing —
  this is shared code the poke matrix depends on, so floor-gate it. Then
  un-ignore prism_chain_diag_27 + "BOOL #27 FIXED (3/3)".
- Reusable bits: `is_point_in_face` planar branch (~11392);
  `point_in_polygon_2d` (~7503); `extract_cycle_vertices_3d`. Edge-usage
  tally trick (no instrumentation): parse the `face_idx=...edges=[...]`
  reconstruct dump under ROSHERA_BOOL_TRACE, count edge-ID occurrences;
  1×=open, 3×=nonmanifold.

Below: original diagnosis detail (still valid).

---

**Boolean #27 — diagnosis (complete).**

- **Read first:** `boolean-coaxial-cap-bug.md` (full diagnosis + fix
  lane) and `fix-fundamentals-first.md` (Varun's strategy steer).
- **Symptom:** building a part by chaining boolean unions (the normal
  CAD feature-stacking workflow) over-reports volume + produces a
  non-manifold solid. Single-extrude parts are flawless; chained
  booleans degrade. Top parity blocker.
- **Repro (clean, fast, in-kernel):** `boolean::tests::prism_chain_diag_27`
  — `#[ignore]`d executable spec. `cargo test -p geometry-engine
  prism_chain_diag -- --ignored --nocapture`. step1 (fresh pair) clean;
  step2 (composite operand) = 135 faces / 61 non-manifold edges / euler
  34 / +7.6% vol / invalid.
- **Root cause (data-confirmed):** the COMPOSITE operand has ANNULAR
  (face-with-hole) faces from the first union; fresh primitives never
  do (→ why fresh unions are clean). Two coupled defects:
  1. `get_face_interior_point` (boolean.rs ~10685) averages OUTER
     boundary midpoints → an annulus's point lands at the CENTRE, in the
     HOLE (trace: ip=(0,0,12)); inner_loops are attached AFTER classify
     so the classifier is blind to the hole → misclassification.
  2. `merge_same_origin_fragments` (~9991) then mis-nests: annulus ends
     with `inner_loops=2`, and a fragment is kept BOTH standalone
     (Union OnBoundary→from_a) AND nested as a hole → invalid
     face-with-hole → non-manifold. Its docstring admits "2-level
     containment only; deep hierarchies mis-route" — composite annular
     faces ARE that case. Also check whether step1 reconstruction leaves
     a PHANTOM inner loop on the composite bottom (the inner_loops=2
     clue).
- **Fix order:** (a) interior point must lie in face MATERIAL (inside
  outer, outside all holes — query `model.faces[original_face]`'s loops
  or test against outer+holes; pick a near-outer-boundary point);
  (b) fix merge multi-level nesting + stop double-keeping;
  (c) verify `prism_chain_diag_27` → valid (mesh_vol ~601k), un-ignore
  it, run the boolean floor, commit, live-re-verify a stepped hub via
  REST, mark task #27 done.
- **Strategy (Varun 2026-06-13):** "don't worry about the [poke] matrix
  — if fundamentals are fixed, future solutions work out of that." Go at
  the face-with-hole fundamental; treat any poke-matrix regression as
  INFORMATION (a workaround that masked the broken fundamental), not an
  automatic veto. Still run the full floor — to learn what moved.
- **Tooling committed:** `ROSHERA_BOOL_TRACE=1` prints per-fragment
  interior point + surface type + selected-fragment lines (commit
  ec8a332).

---

## SHIPPED THIS SESSION (all on main, pushed)

Newest first:
- `ec8a332` BOOL #27: positional fragment trace + root-cause note
- `2be3414` BOOL #27: executable regression spec (`prism_chain_diag_27`)
- `5af3698` SKETCH-DCM: splines + ellipses as profile boundaries
- `15ea55b` MCP: parametric sketching + timeline scrub tools
- `01e2a97` TIMELINE: named checkpoints + non-destructive scrub +
  replayable sketch extrudes
- `e863ad9` ASSEMBLY: bind existing parts as component instances
  (part_id)
- `792c2bc` build-speed: demo profile thin debug + glue crates opt0
  (kernel-change rebuild 8-12min → ~121s incremental)
- `b890cf0` SKETCH-DCM A.2: shared-variable solver (sloppy plate solves
  to EXACT dims)
- `10ab7a8` SKETCH-DCM A.1: csketch→solid bridge + topology walker
  rewrite
- `4d0a099` agent timeline authorship (X-Roshera-Agent header)
- `7214a8d` never-strand WS reconnect + scene resync
- `afa3976` TESS-PERF 85x (extrude 1131ms → 29ms)

---

## BUGS FOUND THIS SESSION (building complex parts flushed them out)

- **#27** chained-union B-Rep (above) — diagnosed, fix pending.
- **#28** REVOLVE: full 2π revolve of a simple offset rectangle →
  `tessellation_empty` (0 verts) on a clean model. Curved-boolean /
  revolve fragility. Repro in `boolean-coaxial-cap-bug.md` BUG B.
- **#29** VALIDATION SCOPE: a new op's post-validation runs over the
  WHOLE model and fails the op when UNRELATED pre-existing solids are
  invalid. Should be scoped to the op's outputs. BUG C.
- Analytic cylinder coaxial union → `InvalidBRep` (curved side faces
  dropped) — task #7 / #16 curved-boolean family.

Tasks #27/#28/#29 are in the session task list.

---

## CAPABILITY STATE (what works / what doesn't)

- **Sketch → solid:** single-extrude parts are watertight + exact
  (engine block, flanged faceplate both verified 0/0/0, volume
  byte-exact). Every kernel sketch entity (line/arc/circle/rect/ellipse/
  spline/NURBS) can now bound an extrudable profile.
- **Parametric sketcher (csketch):** real — 39 constraints, Newton
  solver, shared-variable entity model (A.2), extrude bridge.
  `/api/csketch/*`. NOT yet visible in the browser (Phase E — no
  broadcast/renderer for parametric sketch geometry).
- **Assembly:** real (11 mate types, solver, 102 tests) + part
  instancing (e863ad9). Single global model scene; DOCUMENT MODEL
  (separate part/assembly/drawing docs, multi-window) is the architecture
  Varun wants next — see `assembly-drawing-state.md`.
- **Timeline:** checkpoints + non-destructive scrub + replayable sketch
  events shipped. "better than git" lane started.
- **Chained booleans / feature-stacking:** BROKEN (#27). Build parts as
  single extrudes until #27 lands.

Servers run the `demo` cargo profile (kill api-server before any
`cargo build --profile demo`; first build slow, incrementals ~2min).
`CARGO_BUILD_JOBS=2` for TEST runs only.

---

## WORKING-RHYTHM DIRECTIVES (Varun, this session) — memory feedback files

- `slow-is-smooth.md` — deliberate > fast; read fully before acting.
  (I shipped 2 wrong #27 hypotheses by rushing; both caught + reverted.)
- `birds-eye-after-focus.md` — zoom out after each focused push;
  re-rank before continuing.
- `fix-fundamentals-first.md` — fix the root, don't be gated by
  protecting downstream tests.
- Doctrine for building: fixate → perceive → act → verify (render +
  numeric check every op).

---

## DAY PLAN AHEAD (Varun's stated wants)

- Frontend session: **agent POV panel** (collapsible window showing what
  Claude sees — his idea; becomes the LiveKit verification-layer tile),
  csketch visibility, timeline scrub slider, **document tabs**.
- LiveKit = the verification-layer transport, after the POV panel exists.
- Restart the Claude session to load the new MCP tools (psketch_* +
  timeline_scrub).

---

## RESUME CHECKLIST

1. Read `boolean-coaxial-cap-bug.md` + `fix-fundamentals-first.md`.
2. `git log --oneline -12` (confirm at ec8a332 or later); tree clean.
3. Run `prism_chain_diag_27 --ignored` to re-confirm the repro.
4. Implement the face-with-hole fundamental fix (interior-point → merge
   nesting), verify the repro goes valid, un-ignore it, run the boolean
   floor, commit, push.
5. Then #28 / #29, or the frontend POV-panel work, per Varun.
