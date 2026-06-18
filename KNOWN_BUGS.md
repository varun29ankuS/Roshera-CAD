# Known Bugs — Roshera Kernel

A living ledger of defects found while exercising the kernel through the
API and tests. Append as we find; move to **Fixed** (with the commit)
when closed. The diagnostic render (`GET /api/agent/parts/{id}/render?mode=diagnostic`,
`open_edges`/`nonmanifold_edges`) is the standing oracle for geometry validity.

Status key: 🔴 open · 🟡 in progress · 🟢 fixed

---

## BORE-INTO-REVOLVED-FLANGE CDT PANIC 🟡 SERVER-KILLER FIXED; mesh still drops the hole (2026-06-18)
UPDATE (slice #1, commit 66a3588): the SERVER-KILLER half is FIXED. The
tessellation already wrapped `cdt::triangulate_contours` in `catch_unwind`, but
`profile.release` was `panic = "abort"`, which silently defeated it — so the
first unmeshable face aborted the whole api-server. Set `panic = "unwind"`; now
the cdt panic is caught and the grid fallback runs, so the bored flange is
NON-CRASHING **and** watertight/sound. Gate
`bore_into_revolved_flange_isolates_cdt_panic` (agent_build_eval) green.
REMAINING (🔴 slice-2 finding): the bore is "watertight but WRONG" — the bolt
hole is NOT reflected in the MESH. Bored mesh volume ≈ 508_179 == the un-bored
chamber's mesh volume, i.e. the Ø8 through-hole removed ZERO material. The
boolean-scar CURVED faces around the bore cdt-panic (caught), then the grid
fallback over-covers the region as if solid → the hole is filled, not cut. The
4 panics are on CURVED faces (no planar `[tess] cdt` trace fired), so the next
dig is WHICH face drops the hole and why the fallback plugs it. Pinned repro
(FAILS today, #[ignore]'d): `bore_into_revolved_flange_mesh_reflects_hole` —
bored mesh vol must drop ~1.1k vs the chamber. LESSON (again): watertight ≠
correct — only the VOLUME/effect check caught the dropped hole.

SLICE-3 LOCALIZATION (2026-06-18, ROSHERA_TESS_TRACE instrumentation added to
`curved_cdt::run_cdt` + the cylinder/cone/revolution fallbacks): the 4 panics
are caught in `curved_cdt::run_cdt` with `pts≈295 contours=1 (outer + 0 INNER)
steiner≈253` — i.e. on a curved face with a SINGLE outer boundary and NO holes,
heavily refined (~253 Steiner points). "Failed to create fixed edge" = a STEINER
point lands on the OUTER boundary fixed edge. The Steiner candidate filter
(curved_cdt.rs ~543-575) rejects points near INNER edges (`near_inner_edge`) but
NOT near the OUTER boundary — so a near-outer Steiner triggers the cdt panic.
These are the bore-modified CONE nozzle-band faces (NOT the bolt faces; the bolt
hole is on PLANAR flange faces which meshed without panic). BORE-INDUCED: a clean
revolve / bell nozzle has 0 such panics. Numbers: chamber MESH vol 507_755,
bored MESH vol 508_179 (the bore ADDED +424 instead of removing ~1.1k → off by
~+1.5k). NOTE: the cylinder/cone/SoR UNTRIMMED-grid fallbacks did NOT fire for
these — so `tessellate_curved_cdt` swallows `CdtPanicked` internally (retry/
partial) and emits the wrong-covering tris; tracing THAT is the next dig. FIX
candidates: (1) add a `near_outer_edge` Steiner reject symmetric to
`near_inner_edge` so cdt never gets a boundary-coincident Steiner; (2) make the
post-panic path trim-aware instead of over-covering. Repro unchanged.

SLICE-4 (2026-06-18): fix-candidate (1) FAILED — a `near_outer_edge` Steiner
reject changed NOTHING (panic count 4 → 4, steiner 253 → 253: it rejected ZERO
candidates), so the panic is NOT a near-boundary Steiner. REVERTED it. Extended
the run_cdt panic trace with two probes: `on_outer_edge=0` (no point lies ON an
outer segment → not collinearity) and **`dup_pairs=41 min_pair_dist=4.4e-16`
with outer=42**. So ~41 of the 42 OUTER contour points are FLOATING-POINT-EXACT
COINCIDENT in UV: the face's UV projection COLLAPSES nearly the whole outer
boundary to one location. cdt dedups the coincident points, the outer contour's
fixed edges collapse to degenerate edges, and it `assert!`s "failed to create
fixed edge". ROOT CAUSE = a DEGENERATE UV PROJECTION on these boolean-scar cone
bands (near-zero-area slivers, or a bad projection axis), NOT Steiner placement
and NOT a holed face (0 inner). IMPORTANT PIVOT: these 4 panicking faces are
CONE nozzle bands, NOT the bolt faces — the bolt hole lives on PLANAR flange
faces that meshed without panic. So the cone panic is likely SEPARATE from the
hole-missing. NEXT: (a) trace the bolt PLANAR flange faces — do they carry the
bolt inner loop after the boolean, and does triangulate_planar_polygon mesh the
hole? That is the real volume bug; (b) separately, the cone-band UV-projection
degeneracy is its own curved-CDT robustness bug (dedup the contour / pick a
non-degenerate projection / skip true slivers). Diagnostic kept (env-gated
ROSHERA_TESS_TRACE). Repro unchanged.

SLICE-5 (2026-06-18): CORRECTS slice-4's "separate" guess — the two issues are
CONNECTED. B-Rep face dump (FLANGE-DIAG, env-gated in agent_build_eval) PROVES
the bolt hole IS correctly imprinted: flange-top Plane face (z=200) has inner
loops r=35 (central) + **r=4 @ (50,0) = bolt**; flange-bottom Plane (z=178) has
r=43 + **r=4 @ (50,0)**. Both planar faces mesh WITHOUT panic, so the hole is
present in the mesh. Therefore the volume error is NOT a missing hole — it is the
4 CONE-band panics' fallback adding spurious volume that MASKS the correct bore:
chamber 507_755 − 1_105 (bore) + ~1_529 (cone-panic over-cover) = 508_179 (the
observed bored mesh vol). So fixing `bore_into_revolved_flange_mesh_reflects_hole`
REQUIRES fixing the cone-band degenerate-UV-projection panic (slice-4 root
cause) — it is the real blocker, not a side issue. NEXT: fix the cone-band UV
degeneracy so those faces mesh correctly (no fallback over-cover); then the bore
volume will drop ~1.1k and the repro passes. Approaches: detect the collapsed-UV
contour in curved_cdt and (a) dedup+rebuild the contour, (b) re-project on a
better axis (face normal-aligned basis), or (c) if it is a true zero-area sliver,
emit nothing rather than an untrimmed over-cover.

SLICE-6 (2026-06-18): measured the volume error at BOTH densities (VOL-DIAG, now
reverted): default chamber=507_755 bored=508_179 (delta +424); FINE chamber=
508_006 (== analytic, so the CHAMBER meshes perfectly) bored=497_376 (delta
**-10_629**, want -1_105). So the boolean-scar CONE faces mis-tessellate at
EVERY density — default OVER-COVERS (+~1.5k net) and fine DROPS whole faces
(-~9.5k). The mechanism (refine_to_convergence ~curved_cdt.rs:1238): the initial
run_cdt yields a COARSE triangulation, a refinement re-run then panics on the
degenerate-UV contour and FREEZES on that coarse mesh (default), while fine
params push the initial call itself to fail -> face dropped. The CHAMBER's cone
bands mesh fine (508_006 exact) — only the BORE-MODIFIED bands break, so the
boolean is MODIFYING the cone faces' boundary loops (corefinement adds vertices)
in a way that makes their UV projection collapse. REFRAME: the SOLID is correct
(sound B-Rep, hole imprinted, validate_solid_scoped passes) — only the DISPLAY
MESH of boolean-scar cone bands is broken, so EXPORT/mass-props off the B-Rep are
fine; the live-viewport/STL mesh is wrong. This is a DEEP curved-CDT robustness
bug needing a dedicated fix (not a one-loop-slice patch) + a full curved-suite
regression. The mesh-volume repro is UNSTABLE as written (density-dependent);
the real fix target is the curved-CDT projection/refinement for boolean-modified
cone bands. Server-killer (slice#1) remains the shipped win.

SLICE-7 (2026-06-18): attempted the bounded fix — a collapsed-projection guard
in `curved_cdt::validate_loop` (reject a loop whose projected points are >50%
coincident -> DegenerateLoop -> analytic-grid fallback). It was a NO-OP: panics
stayed 4->4, volume unchanged. So the panicking CONE faces DO NOT go through
`run_boundary_projection`/`validate_loop` at all — `tessellate_conical_face`
(surface.rs ~2967) has its OWN projection + run_cdt path that bypasses the
generic curved_cdt Step-0 validation. REVERTED the guard. CONCLUSION: the
boolean-scar-cone mesh fix is a DEDICATED effort (trace + fix the cone-specific
tessellation dispatch, then add the collapse guard at THAT path's projection),
not a self-paced loop slice. It is a DISPLAY-mesh bug (solid is sound), so it is
lower priority than shipped correctness. PIVOTING the loop to the revolve pole
apex-fan (eval_revolved_dome, #[ignore]'d) — a more self-contained curved item.
The 6 #24 commits (server-killer fix + full diagnosis) stay on main.

(Original report below — the abort/crash mechanism, now fixed.)

## BORE-INTO-REVOLVED-FLANGE CDT PANIC 🔴🔴 SERVER-KILLER (2026-06-17)
Differencing a small axial cylinder (bolt hole) out of a REVOLVED solid's
annular flange PANICS the kernel and — because the release profile is
`panic="abort"` — takes the whole api-server process down:
```
thread 'tokio-runtime-worker' panicked at cdt-0.1.0/src/triangulate.rs:965:
Failed to create fixed edge
```
Repro: revolve the thrust-chamber profile
`[[70,0],[50,30],[30,60],[15,90],[25,105],[35,120],[35,200],[58,200],[58,178],[43,178],[43,120],[33,105],[23,90],[38,60],[58,30],[78,0]]`
(integral flange → annular planar top z=200 r35-58 + bottom z=178 r43-58), then
`difference` a Ø8 cylinder at radius 50. Reproduced BOTH with a tall cylinder
(z0-210, also pierces the nozzle cone) AND a short flange-only cylinder
(z170-205, custom plane origin) AND at full `default()` tessellation — so it is
NOT the cone and NOT tessellation coarseness. Root: the boolean RESULT's planar
flange face becomes an annulus carrying a SECOND, non-concentric hole (the bolt),
and `cdt` fails to insert a constrained (fixed) hole-boundary edge → panic. Same
#24 curved/planar-CDT-with-holes family. Simple booleans (box ∖ cylinder = bored
plate) still work; it is specifically the bore into a revolved annular flange.
TWO fixes needed: (1) harden the planar-CDT path for an annular face with an
offset hole (the actual bug); (2) panic-ISOLATE the broadcast/perception
tessellation so a kernel panic can't kill the server — but release is
`panic="abort"`, so `catch_unwind` is a no-op; either build api-server with
`panic="unwind"` or tessellate in a process/worker that can't abort the server.
Until fixed: do NOT bore bolt circles into a revolved flange on the live server.
NOTE: the coarse `display()` preset added 2026-06-17 AMPLIFIED a related curved
case; it was REVERTED (all three api-server `tessellate_solid` broadcast sites
back to `default()`); the `display()` preset definition remains (unused) in
`tessellation/mod.rs`, blocked on this fix + panic-isolation. The
`perception_json` watertight-from-B-Rep decouple (api-server/src/main.rs) was
KEPT — it is mesh-independent and correct.

---

## Tessellation

### #51 🟢 FIXED — short-protrusion boss tessellated non-manifold (valid B-Rep)
**RESOLVED by TESS-PERF #58 (2026-06-14).** A box + interpenetrating cylinder
boss union whose EXPOSED protruding wall was short (≤ ~8mm) yielded a VALID
B-Rep that tessellated NON-MANIFOLD (`open=0`, `nm = 2×angular-segments`). The
report called it "chord-independent / structural / not a weld-tolerance issue" —
correct that it wasn't density, but it WAS a tessellation-TOPOLOGY artifact: the
curved-CDT Ruppert skinny pass over-refined the short developable wall and its
rim didn't weld. #58 (developable Steiner collapse + fidelity-gated skinny
refinement) cleared it. Verified across the whole formerly-failing range
(exposed wall 1–8mm × r6/r12): `box_boss_short_protrusion_tessellates_manifold_51`
now passes (un-#[ignore]'d, upgraded to a sweep guard), and the HARNESS-1000
box-boss grid restored to `boss_h=[10,25]` (short bosses re-covered) stays green.
LESSON: a "valid B-Rep but non-manifold mesh" pin is by definition a tessellation
artifact — several were #58 collateral (cf #84). See memory
`flanged-84-35-corefinement.md`.

---

## Section / clip

### #85 🟢 Axial cylinder section (plane containing the axis) returns no caps
FIXED (f9df69a). An AXIAL cut of a cylinder (plane contains the axis) must give
a `2r·h` rectangle but returned NO caps. The lateral cylinder∩plane = two
straight generator lines (the marching SSI finds these fine); the actual gap was
the END CAPS. A disc cap is bounded by a SINGLE closed circular edge whose
start_vertex == end_vertex (seam), so `loop_points` (one vertex per directed
edge) yielded ONE point; `plane_face_fragments` saw `n < 2` crossings and
emitted nothing, so the two cap diameter segments were lost and the rectangle
never closed. (Never exercised before: a radial cut produces the circle on the
lateral face and never touches the caps.) Fix: `loop_points` samples curved
boundary edges (`Edge::evaluate`, 64 interior pts when `!curve.is_linear`) so
the planar clip sees the real circular polygon; straight edges unchanged (line =
its start vertex, preserving #83's exact box clip). Guards:
`section_cylinder_axial_plane_produces_rectangle_85` (lib) +
un-ignored `axial_cylinder_section_returns_none_85` (integration, render_section).

### #83 🟢 `section_solid_by_plane` ignores PLANAR faces (plain box → 0 caps)
**FIXED** (research-grade, EYE-2 lane): the marching-square SSI fragmented a
single straight cut line into 2 disjoint pieces on WIDE/SHORT planar faces (box
sides where the in-plane span ≫ the cut-direction span), so the 8 pieces never
chained into a cap — cubes (equal extents → 1 clean piece/face) accidentally
worked, masking it. Fix: in `collect_face_fragments`, branch `Plane` faces to an
EXACT Plane×Plane clip — the two planes meet in a line `p₀ + t·(n_cut×n_face)`
(closed-form `p₀`), clipped to the face by even-odd crossings against every loop
edge (outer + holes); curved faces keep marching. Guards: `section_planar_box_dims_match_analytic`
(every aspect ratio → area=w·h), `render::dimensioned::section_planar_faces_covered_83`
(bored plate → 3600−100π), existing cube/oblique/cylinder tests still green.
Original report:
Found by EYE-2 (the section render, dogfooding `render_section`). Sectioning a
plain box returns ZERO caps; a bored plate returns only the bore disk
(area 314.15 = πr², missing the 60×60 outer square). Diagnosis
(`diag_section_caps`): plain-box=0, plain-cyl=1 (correct disk), bored-plate=1
(only the cylinder loop). Root: `collect_face_fragments` →
`intersect_surface_plane` (generic marching-square SSI) produces no zero-crossing
fragments for **Plane** faces — likely the face UV-bounds
(`get_face_parameter_bounds` returns the [0,1] placeholder for analytic faces)
or the Plane parameterization leaves the signed-distance grid without a sign
change. Curved faces (cylinder/sphere/cone) section correctly. Impact: section /
clip is unusable on any part with planar faces (≈ all mechanical parts) — only
curved cross-sections work. The EYE-2 RENDER layer (`render_section` +
`SectionFrame`) is correct and ready; it lights up once this is fixed. Repro:
`render::dimensioned::tests::section_planar_faces_missing_83` (#[ignore]). Fix is
in the kernel SSI / Plane face-UV-bounds — fresh-context (section.rs is 1227
LOC).

---

## Blend (fillet / chamfer)

### #82 🟡 Multi-edge blend on adjacent (corner-sharing) edges not implemented
Found by the ribbed-bracket S3 build. `fillet_edges` / `chamfer_edges` over
edges that share a corner vertex where the *selected* edges meet at a degree-2
`ConvexCorner` (e.g. the 4 top-perimeter edges of a box, meeting pairwise at the
4 top corners) returns `NotImplemented`: "corner-patch synthesis for this vertex
kind is not yet implemented (Task #82 / F5-γ / F5-δ). Apply each edge in a
separate call." This is the kernel's own internal **#82 / F5-γ/δ** corner-patch
work — a known unimplemented feature, NOT a regression. The earlier multi-edge
corner fixes (#51/#62/#63) covered all-fillet 3-edge box-VERTEX corners and
multi-edge chamfer; this degree-2 face-perimeter `ConvexCorner` is the remaining
gap. **Supported path:** blend only vertex-disjoint edge sets — e.g. the 4
vertical edges of a box are pairwise disjoint, so `ribbed_bracket`
(parts_invariant_sweep.rs) fillets 2 + chamfers 2 of them cleanly. Repro:
`multi_edge_adjacent_fillet_unsupported_82` (#[ignore]); flip on when F5-γ/δ
lands.

---

## Boolean

### #34/#80 🔴 box∘box difference bbox over-inclusion (robustness ceiling)
Discovered by the tier-3 bbox-containment proptest
(`prop_tier3_difference_bbox_within_minuend`) during the BOOL #7 cyl×sphere fire;
proven PRE-EXISTING (reproduces with that fire's changes stashed). `A ∖ B` must
satisfy `bbox(A∖B) ⊆ bbox(A)` (subtracting can't grow the minuend), but for a
near-degenerate config where B slightly exceeds A in some axes, the result bbox
escapes A — the difference keeps B's larger extent instead of clipping to A
(classic over-inclusion). Shrunk case (boxes centred at origin):
A = 19.828×19.814×8.401 ∖ B = 2.000×19.852×8.402 → result max.y 9.926 > A's
9.907. Explicit pin: `box_box_bbox_overinclusion.rs::
box_box_difference_bbox_within_minuend_3480` (#[ignore]). The randomized tier-3
proptest remains the live discovery gate (usually green; occasionally re-finds a
case). Fix lane = the #34/#80 over-inclusion class (classification keeps
material outside the minuend); DEEP, ties exact predicates (#30). NOTE: NOT
caused by the cyl×sphere arm (7666c2e) — that was verified independent.

### MARCH-HANG 🟢 FIXED — curved×curved booleans with no analytic SSI arm froze the kernel
Live dogfood ("union of cone and cylinder takes a loooot of time") = a TRUE HANG
(>25s, no return) on every cone∪cylinder config, even a trivial coaxial one. A
hang freezes the whole api-server (worst failure class). Root cause: cone∘cylinder
(and cone∘sphere, etc.) have no analytic SSI arm → `march_surface_intersection` →
`march_from_point`, whose loop had NO iteration cap and a step tied to the 1µm
distance tol (~1.5M steps/unit-curve), made quadratic by `insert(0,..)`. FIX
(13e3f5a): hard cap `MAX_MARCH_STEPS=200_000` (discard the curve as unreliable
past it — `Ok(None)`) + O(n) splice instead of `insert(0,..)` + closure test vs
the seed. cone∪cylinder now RETURNS in ~3–4s. NOTE: this stops the freeze only;
those pairs are still geometrically WRONG (marched curve discarded) until their
analytic SSI arms land (task #7). Guard:
`cone_cyl_hang_probe.rs::cone_union_cylinder_terminates`.

### #7 🟡 cylinder ∖ sphere — analytic cyl×sphere SSI (campaign, live-surfaced; 1 of 2 fixed)
Surfaced by a live dogfood ("subtract a sphere from a cylinder of the same
radius"). `surface_surface_intersection` has no Cylinder–Sphere arm → routes to
the generic MARCHING fallback. A z-cylinder centred at origin (r_c, h=10) minus a
sphere at origin (r_s); for r_s ≤ r_c, r_s ≤ 5 the sphere is fully enclosed, so
the result should be the cylinder with a spherical cavity (vol = π·r_c²·10 −
(4/3)π·r_s³, watertight, valid 2-shell solid). Two distinct failures (reproduced
offline AND via the live api-server, identical numbers):
- **Enclosed void (r_c=5, r_s=4): 🟢 FIXED (462e4ca).** Geometry was always
  CORRECT — watertight mesh, volume −0.0% — but the B-Rep validator wrongly read
  `Euler χ = V(2)−E(3)+F(4) = 3 odd`. The kernel models a sphere as a single
  SEAMLESS closed face (χ=1, not a disk); the validator accepted that for a lone
  sphere (e==0 guard) but its multi-shell Euler sum undercounted the seamless
  void face by 1. Fix: count seamless closed faces (zero bounding edges) and add
  +1 each to the Euler sum (each closed-surface face is χ=2). Gate
  `cyl_minus_sphere_enclosed_void_7` now passing.
- **Same radius (r_c=r_s=5): 🔴 still open.** The sphere is TANGENT to the
  cylinder wall along the whole equator; the intersection degenerates to a tangent
  circle the marcher can't trace → **200 open edges**, not watertight, invalid.
  The deep case — needs the analytic cyl×sphere SSI (coaxial d=0 → 0/1/2 circles;
  r_s=r_c → single tangent circle, no material removed across tangency) + tangency
  handling. Pin `cyl_minus_sphere_same_radius_7` (#[ignore]). The api-server
  perception block self-reported `valid:false/watertight:false`
  (feedback-as-default working). Ties task #7.

### #1 🟢 Cone-radial conic-cut — FIXED (18/21 cells; 1 sub-case remains)
A z-axis cone shifted off-axis so its slanted LATERAL surface pierces a box side
wall; the cone × plane section is two generator lines (wall ∋ axis) or a
hyperbola (offset). **ROOT CAUSE:** `split_face_by_curves` had a sector splitter
for the CYLINDER lateral (`split_cylinder_lateral_by_sectors`) but NONE for the
CONE — so cone generator cuts fell through to the generic DCEL, which can't
partition the periodic cone u-domain and dropped the inside angular strip (x<1,
cone angles ≈90°–270°); ∩ then kept only the 3 planar faces. The SSI was already
correct (`cone_plane_ssi_points_lie_on_both_surfaces_1`) — purely the curved-face
arrangement. **FIX:** added `split_cone_face_by_sectors` (cone analogue: axial =
distance-from-apex, rim radius = v·tan(half_angle)). Cone HARD fuzz cells
21 → 3; `radial-face+x` and `radial-edge` now exact (∩/∪/∖ watertight + valid +
volume within 0.0%). Verified no regression: lib 3721/0, poke_matrix 33/33,
determinism/adversarial/volume-proptest, HARNESS-1000 13/0. GATE:
`boolean_fuzz_survey.rs::cone_radial_conic_cut_gate_1` (now passing, both cells).
**REMAINING (still 🔴, tracked under #1):** `radial-poke-past` (bc=[1.4,…], cone
base ENTIRELY outside the box) — ∩ vol +199%, open=2; ∖ nonmanifold=2; a distinct
sub-case (the base rim, not just the lateral, exits the wall). Fix lane: extend
the sector handling for the base-outside topology — DEEP, follow-up. The 33-cell
curved poke matrix (`harness::poke_matrix`) stays fully green throughout.

### #86 🟢 FIXED — 6-boss mount-plate chained booleans appeared to HANG the kernel
**Root cause (2026-06-14): NOT a boolean bug and NOT an infinite loop — it was
catastrophically-slow but finite OPERAND TESSELLATION, surfaced through the
boolean classifier.** BOOL-ARCH-2's generalized-winding-number classification is
default-ON; `classify_point_relative_to_solid` → `classify_point_gwn` →
`solid_gwn_triangles` tessellated each operand with `TessellationParams::default()`
(display-FINE). A single boss cylinder lateral (r9 h20) tessellates to ~20 000
triangles in ~4 s via the curved-CDT Ruppert refinement; a 6-boss husk has ~12
cylinder faces, so one operand tessellation ≈ 30–40 s and the chained build ran
for minutes → "wedged" (the ~1971 CPU-s observed live). Earlier handoffs
mis-pinned this to `extract_regions` degenerate loops / `classify_split_faces`;
instrumented `ROSHERA_BOOL_TRACE` + `ROSHERA_TESS_TRACE` traces proved it is
`tessellate_solid` on the operand.
**FIX:** `solid_gwn_triangles` now uses `TessellationParams::coarse()` (the mesh
feeds only the winding SIGN, never display/export; near-boundary points are
resolved upstream analytically by the `is_point_in_face` coincident-loop before
GWN runs). The 6-boss build: >120 s hang → **10.87 s, all 13 stages**.
`bool86_hang_isolation` (#[ignore]) now PASSES (asserts termination). Verified no
classification regression: poke-envelope (exact), determinism, brep-oracle,
adversarial-intersection, volume-proptest, analytic-watertight, curved-CDT — all
green. **Downstream TESS-PERF also FIXED (same session):** the cylinder/cone
DISPLAY over-tessellation that this exposed (curved-CDT over-refining the
developable lateral) is fixed in `curved_cdt.rs` — developable-direction Steiner
collapse + skinny-refinement gated on geometric fidelity. Boss cylinder
20202 tris/4.5s → 2872/58ms; HARNESS-1000 ~975s → 56s; all tessellation +
boolean suites green. See memory `bool86-gwn-tessellation-hang.md`.

### #84 🟢 FIXED + #35 🟢 FIXED — coaxial through-pierce flanged body, fully clean
**The headline bug — coaxial through-pierce UNION → non-manifold — is RESOLVED**
(2026-06-14). It was a TESSELLATION ARTIFACT, not a B-Rep defect: the old
over-refined cylinder lateral didn't align with the flange-cap annulus sampling,
so the *mesh* had T-junctions (nm=72) at the pierce rim. The TESS-PERF #58 fix
(developable Steiner collapse + fidelity-gated skinny refinement) made the seams
align. `diag_flanged_stages` now shows `A:body+flange union: open=0 nm=0
brep_valid=true faces=7`, central bore and 1st bolt likewise clean — verified at
the B-Rep level (`validate_solid_scoped`, mesh-independent), not just the mesh.

**Residual #35 — FIXED 2026-06-14 (commits 98c20c5 + d4b5113).** A chained
DIFFERENCE into a cap that already has holes dropped the new (and, for 3+ holes,
a pre-existing) hole, orphaning its wall as a separate shell. ROOT was the SAME
chord-polygon flaw in two stages, both using boundary-edge ENDPOINTS for a
strict point-in-polygon containment test: (1) `merge_same_origin_fragments`
(absorbing the new cut's disc as an inner loop) and (2)
`partition_outer_and_pre_existing_hole_cycles` (re-attaching pre-existing holes
to their outer). For a curved cap rim split into ~3 arcs the inscribed chord
triangle's incircle is ~r/2, so a hole at radius 30 in an r40 cap tested OUTSIDE
the outer and was dropped. FIX: build both containment polygons by SAMPLING each
boundary edge's curve (8/edge, follow arcs) — mirrors `is_point_in_face`. Now
diag_flanged_stages: union/bore/bolt0..bolt3 ALL `open=0 nm=0 brep_valid=true`
(bolt3 faces=14). `flanged_body_verify_dimension` un-#[ignore]'d as a running
guard; verified no regression (118 boolean lib + poke + volume + tess +
HARNESS-1000). NOT a vertex-tolerance issue (near-miss probe: zero pairs in
1e-6..0.5). Pinned tests now PASS; `diag_flanged_stages` (#[ignore], slow) keeps
the per-stage characterization.

### #85b 🟢 FIXED — section of a multi-hole planar profile gave wrong area
Surfaced when #35 made the 4-bolt flange valid. Root cause was NOT cdt (red
herring): `section::point_in_polygon` mis-classified loop nesting. Its even-odd
ray-cast denominator `(yj-yi).max(1e-18).copysign(yj-yi)` clobbered any NEGATIVE
dy to ±1e-18 (missing `.abs()`), so the x-intersection blew up on every DOWNWARD
edge → PIP wrong for any polygon with downward edges (every circle) →
classify_loop_nesting read the cap's 4 bolt holes + centre bore as separate solid
discs (area 5441 vs analytic 4511, +20%). FIX (bfbdf4d): keep magnitude AND sign,
`(yj-yi).abs().max(1e-18).copysign(yj-yi)`. Now the r40 outer owns all 5 inner
circles as holes. `flanged_body_section_multihole_85b` un-#[ignore]'d (running
guard); section_area_sweep + 82 section lib tests green.

### #41b 🟢 FIXED 2026-06-17 — resolved by the EXTRUDE-CYL-MESH-INVERTED orientation fix
The extrude-path boss-wall drop was a downstream symptom of the inverted extrude
cylinder lateral (EXTRUDE-CYL-MESH-INVERTED, below): the boss wall's inward
orientation made the coaxial difference mis-handle/drop it. With
`create_side_face_shared` now orienting side faces from the surface sample point
(commit 3fc8fdb), the extrude boss lateral winds outward and the bore keeps it.
GATE `agent_build_eval::extrude_boss_coaxial_bore_keeps_wall` (PASSES): box base
∪ EXTRUDE-circle boss − coaxial through-bore → valid + watertight + Ø70 boss wall
present (was 300 open / invalid / wall dropped). The analytic-boss path was
already sound (`bearing_housing_coaxial_bore_is_sound`). Original report below.

### (orig) #41b — coaxial bore through a boss STILL drops the outer wall (live)
Live dogfood after the TESS-ANNULAR-CAP fix: built a bearing housing —
base 120×120×20 ∪ boss (MCP create_cylinder r35, z10→50, interpenetrating),
then − coaxial bore (r20, through). Result: `sound:false / valid:false / 300
open / "B-Rep invalid (a real topological defect)"`, bbox z=55 (the cutter top,
above the boss z=50 — a dangling bore wall) and the boss OUTER wall (r35) is
DROPPED (diagnostic render: see straight through the boss, open rim at its base).
This is the SAME signature as #41 (below, marked fixed for analytic
create_cylinder_3d via interior-point-projection): the boss wall's interior point
averages toward the axis → classified Inside the r20 bore → wall dropped. The fix
evidently does NOT cover this config — likely because the MCP/extrude-path boss
(or the boss after a UNION onto the base) is not the concrete analytic `Cylinder`
the projection special-cases, OR because the union changed the wall face's loop so
the centroid-projection no longer lands on it. NOTE: the SOUND verdict CORRECTLY
flagged this BROKEN (the EYE-SOUND channel works — contrast the bored-plate
false-green which had no volume gate). Repro: REST `POST /api/geometry` box +
`POST /api/geometry/cylinder` boss + `/api/geometry/boolean` union + bore; or pin
a kernel test mirroring it. Fix lane: extend the #41 interior-point projection to
cover boss walls that are not the concrete analytic Cylinder (project onto the
face's actual surface, or use GWN classification for the wall face), tying to the
#41/#35 coaxial-bore family. Workaround to keep building: bore BEFORE the union,
or keep features non-coaxial.

### COAXIAL-BORE-THROUGH-BOSS 🟢 FIXED 2026-06-17 — it was the annular-cap stitch, NOT corefinement
**The boolean was always CORRECT** — NOT a #35/#41 corefinement defect (an
earlier "corefinement" framing was wrong, itself a retraction of a still-earlier
false "sound" pass). Per-face inspection of base ∪ boss − coaxial bore proved the
B-Rep is fully bored: bore wall through the base (z[-10,10]) AND the boss
(z[10,40], both oriented toward the axis), and the boss-top cap is annular
(inner loop = the bore). The failure was purely TESSELLATION: the concentric
boss-top annulus (outer rim + inner bore — independent seams, opposite winding)
went through `annulus_radial_strip`, which stitched the two rings by fractional
INDEX assuming a common seam + same winding (true for revolve washers, false
here) → the strip twisted into overlapping spanning triangles that FILLED the
bore (mesh area 5484 vs the true annulus 2591). So the boss rendered SOLID and
the mesh volume reflected only the base bore (removed ≈24244 vs the full ≈62832).
FIX: angle-ordered stitching in `annulus_radial_strip` — reorder each ring into
canonical CCW-by-angle order about its own centre (kills the winding/seam
dependence) and rotate the inner ring to align its first point with the outer's.
RESULT: boss-top area 5484→2592, removed 62805 ≈ full bore, bore now goes through
the boss (render shows the hole). Gate `bearing_housing_coaxial_bore_is_sound`
(un-ignored, VERIFY-EFFECT volume assertion PASSES). NO regression:
revolve_watertight 7/7 (washers — the other annulus_radial_strip client —
unaffected, since angle-order is a no-op for their aligned rings),
primitive_tess, tess_seam_65, bore_rim 5, drawing 36, surface lib 18, eval 11.
THE LESSON (yet again): only the VERIFY-EFFECT volume check caught this; "valid +
watertight + a feature face exists" passed on a filled bore. The #41b wall-drop
was a separate, also-fixed defect.

(superseded) UPDATE 2026-06-17 — claimed the KERNEL corefinement is SOUND; the
live failure a PIPELINE artifact. Reproduced the analytic config in-kernel:
validate_solid_scoped VALID, watertight, boss-top inner loop present —
but this MISSED that the boss interior column is never bored (volume check added
later proves removed≈24244 ≠ full 62832). NEXT: trace the
LIVE difference (ROSHERA_BOOL_TRACE / TESS_TRACE) on a clean rebuild to find why
the live result diverges from the (correct) kernel result. The EXTRUDE-path boss
(below) is still a genuine bug.

**ORIGINAL (extrude-path) FAILURE — still a real bug:**
- EXTRUDE-path boss (MCP create_cylinder = sketch+extrude): boss OUTER wall
  dropped → 300 open, B-Rep invalid, bbox z overshoots to the cutter top.
- ANALYTIC boss (POST /api/geometry/cylinder = create_cylinder_3d): much closer —
  mesh WATERTIGHT (open=0 nm=0) with CORRECT dims (120×120×50), boss wall PRESENT
  — but the boss TOP cap is left a SOLID disc (the coaxial bore never opens it;
  `ids` top = full Ø70 green disc, no Ø40 hole) ⇒ B-Rep `valid=false`. The bore
  opened the base but not the boss top: the boss-top annular cap / the wall↔cap
  imprint at the bore exit is missing. Same #35 corefinement signature (the two
  coaxial walls' intersection not imprinted into both faces).
- ALSO: tessellating the analytic version at FINE density panics the `cdt` crate
  (triangulate.rs:1015) — the known #24 curved-CDT spanning-triangle panic.
Kernel repro: `agent_build_eval::diag_bearing_housing_boss_wall` (#[ignore]) —
base ∪ boss − coaxial bore; analytic gives valid? + watertight but the panic
fires at fine tess. Fix lane = the #35/#41 coaxial corefinement (imprint the
shared bore↔boss-wall intersection into BOTH faces; persist the boss-top annular
hole), DEEP — same lane as the #27 chained-union work. NET for the agent: prefer
the ANALYTIC cylinder primitive (watertight, wall kept) over extrude, and avoid
bores that exit through a boss top until corefinement lands.

### #41 🟢 Coaxial bore through a cylindrical boss dropped the outer wall
Found live (ladder step 6, bearing housing). `plate ∪ analytic-cylinder boss`
(r30, interpenetrating) is CLEAN (open=0). Differencing a COAXIAL analytic
cylinder bore (r15, same axis, through) → 600 open: the boss's OUTER wall
(r30) is dropped entirely (diagnostic: boss top rim + base seam open, you see
through where the r30 wall was; the r15 bore wall is intact). The r30 wall is
wholly OUTSIDE the r15 bore → must be KEPT. A plain box−cylinder bore is
clean, so the trigger is the bore being coaxial/concentric with a pre-existing
analytic CYLINDER wall in the target. Likely root: difference face-
classification / point-in-cutter membership mishandles a coaxial cylinder
wall vs a cylinder cutter.
**ROOT CAUSE + FIX (trace-confirmed):** NOT an SSI bug — it was the interior
point. `get_face_interior_point` averages boundary-edge midpoints; for a
cylinder WALL the boundary (top+bottom circles + seam) averages toward the
AXIS, so the outer r30 wall's interior point came out at (7.5,0,20) — inside
the r15 bore — and the wall classified `Inside` and was dropped. Fixed by
projecting the centroid back ONTO the analytic surface for
Cylinder/Sphere/Cone faces (closest_point → point_at): the interior point is
now (30,0,20) → `Outside` → wall kept. Minimal repro
`boolean::tests::diff_coaxial_cylinder_tube_41` now open=0/nm=0. Suites green:
boolean 98, curved_boolean_poke_envelope 4, determinism 3, operations 685.

### #35 🟡 Difference cut intersecting another bore leaves open faces
NOTE (2026-06-13): the #41 interior-point fix did NOT help #35 — re-checked
faceted (15 open / 3 nm) and analytic (600 open) both still broken. Confirms
#35 is the saddle IMPRINT/WELD where the two cutters' walls cross (a
corefinement problem), NOT the curved-face interior-point classification that
#41 fixed. Stays the deep corefinement lane (#30/#6).

Box − vertical bore − crossing horizontal bore: the second cut's wall
fragments where it breaks into the FIRST bore's void.
**CONFIRMED for ANALYTIC cylinders too (2026-06-13, ladder step 5):** block
− analytic vbore (clean, open=0) − crossing analytic hbore → 600 open edges;
the vertical bore is visibly shattered open at the saddle (diagnostic
render). So this is NOT a faceting artifact — the cylinder∩cylinder saddle
weld fails the same way, which means the COREFINEMENT fix (shared
intersection vertices) is the right cure for both facet and analytic paths.
Repro:
`boolean::tests::diff_intersecting_bores_35` (ignored) → 15 open + 3 nm,
euler −10. **Localized (diagnostic render, 2026-06-13):** the open edges
are the saddle intersection curve where the horizontal tunnel wall meets
the vertical bore void — the hbore wall facets that pass through the
already-empty vbore region are not trimmed/welded against the vbore wall
facets, leaving the saddle loop open. Both bores are 24-gon prisms, so
this is faceted plane∩plane at the saddle, not analytic SSI.

**Root cause refined (2026-06-13, deeper trace + experiment):** NOT a
weld-tolerance problem. Both walls ARE split at the saddle (2nd diff:
51 kept solid-A frags + 48 kept solid-B frags), but each operand imprints
the saddle intersection INDEPENDENTLY, producing vertices that are not
coincident — `canonicalise_face_edges_by_position` reports
`canonical_collapses=0` even at a 1e-3 probe tolerance (1000× the model
1e-6). So the kept A-wall fragments and B-wall fragments meet along two
slightly different polylines → genuine gap → 15 open edges. Position-weld
can't fix it because the points genuinely differ.

**Fix direction — COREFINEMENT (cutting-edge consensus):** compute each
face-pair intersection ONCE and insert the SAME shared vertices/edge into
BOTH faces' splits, rather than letting each operand re-imprint. Two
robustness substrates from current literature:
- **Indirect predicates** (Cherchi/Livesu/Attene, "Interactive and Robust
  Mesh Booleans" SIGGRAPH 2022; "Fast and Robust Mesh Arrangements" SIGGRAPH
  Asia 2020, header-only OSS): represent an intersection vertex IMPLICITLY by
  its defining construction (the 3 planes that meet there), so both faces
  reference the identical point by construction — coincidence is exact, not
  tolerance-based. Ties to #30 (exact predicates).
- **CGAL-style corefinement**: exact constructions under the hood, both
  surfaces refined along one shared polyline.
Roshera is a B-Rep (not a triangle soup), so the targeted version: in
`compute_face_intersections` / the split, intern the intersection curve's
vertices in a shared table keyed by the implicit construction and reuse the
same VertexId/EdgeId when imprinting both operands' faces. Ties to #6
(persist boolean pcurves) and #30. Refs: arXiv:2205.14151, arXiv:2405.12949,
ACM TOG EMBER (10.1145/3528223.3530181).

### #36 🟢 Boolean leaves invalid operand husks in the solid store
After a boolean the consumed operands lingered in `SolidStore` as
degenerate solids (Euler χ ≠ 2) — phantom parts that amplified #29.
Fixed: `boolean_operation` now removes both operands from `SolidStore`
on success (inside the rollback closure, so a failure restores them).
This aligns the kernel with the API, which already unregisters the
operand UUIDs and broadcasts `object_deleted`. Verified live: a union's
`GET /api/agent/parts` now lists only the result. Commutativity parity
tests updated to `deep_clone` operands for the second ordering.

### #32 🟢 FIXED — coincident-face union (Same-Domain) (commit 450fb77)
Unioning a solid whose face sits exactly coincident on another's (a riser on a
plinth top; a boss flush on a box) yielded χ=odd + non-manifold rim (3 faces per
edge). ROOT: the pipeline imprinted+split+classified the coincident face
(OnBoundary) correctly, but selection's `OnBoundary → from_a` kept the buried
disc — which is sandwiched between the two operands' interiors (anti-coincident
= INTERNAL). FIX: a Same-Domain cull (`cull_internal_coincident_faces`, between
merge and select): probe one side of each OnBoundary face and classify vs BOTH
operands; `inside(A) XOR inside(B)` ⇒ opposite sides ⇒ internal ⇒ drop;
same-domain kept (selection dedups). Orientation-independent. Coincident
face-to-face union now works (no more interpenetrate-only workaround). Running
guard: `box_boss_coincident_base_union_valid_32`. Verified no regression across
118 boolean lib + poke + volume + adversarial + determinism + oracle +
HARNESS-1000.

### #27 🟢 Coaxial stacked-step union left buried cap
Fixed via annular face-with-hole interior-point. See boolean campaign.

### #27/#32 cone family 🟢 FIXED — coincident cone rim left unwelded ("rocket") (commit ae1c8ad)
A cone stacked/offset on a cylinder so the cone base circle is COINCIDENT with a
cylinder rim (Varun's live "rocket with nozzle at the bottom") unioned to a
watertight-LOOKING solid with EXACT volume (916.30) but a hollow B-Rep: 279 open
edges, invalid. The correct volume + clean render hid it. ROOT: the cone
primitive placed its rim seam VERTEX at `center + axis.perpendicular()*r` (+Y for
a −Z internal axis) while the rim Circle/Arc parametrizes t=0 at the canonical +X
(±Z→+X, ±X→+Y, ±Y→+Z; see `Arc::new`). The full-cone rim edge's `param_range
[0,1]` therefore did NOT start at its `start_vertex`, so
`heal_t_junctions_across_faces` saw the coincident foreign vertex land on the
param boundary (t=0) and could not split the closed circle → rim welded on the
cylinder side only. (The cylinder is immune — it already uses `Circle::x_axis()`.)
FIX: derive cone `ref_dir` from `Circle::x_axis()` so surface seam, rim curve
t=0, seam vertex, and edge param_range all coincide; plus a periodic-wrap guard
in the T-junction healer. Same fix ALSO closed the coincident cone-base
difference. Result open=0/valid=true. Gates: `cyl_union_cone_stacked_rocket_27`,
`cyl_minus_cone_coincident_base_7` (flipped ignore→live). Verified lib 3724/0 +
cone/cyl/sphere suites + poke 14/15. LESSON: a closed-curve (seam) edge's
`param_range.start` MUST sit at its `start_vertex` (= `curve.evaluate(0)`).
See memory `cone-rim-seam-alignment.md`.

### #27/#32 frustum throat 🟢 FIXED — coincident closed-circle rims not welded (commit 7af8e4e)
Sibling of the cone "rocket": surfaced building a de Laval rocket nozzle via the
API (`convergent r6→r20 ∪ divergent r18→r6` sharing the r6 throat circle). The
union was watertight (open=0) with EXACT volume (15808) but B-Rep INVALID — odd
Euler V(3)−E(6)+F(4)−R(0)=1 because the throat was TWO unmerged closed-circle
edges. Unlike the rocket (closed-circle vs arcs, healed by a T-junction split),
here BOTH rims are full closed circles sharing the SAME seam vertex → no foreign
vertex to split, and `canonicalise_face_edges_by_position` SKIPPED all
closed-circle edges (`cs == ce`). FIX: canonicalise now welds genuine coincident
closed-circle edges (skip narrowed to degenerate edges via `start_vertex !=
end_vertex`; (X,X) bucket key discriminated by the circle's antipode midpoint).
Gate `frustum_union_frustum_throat_27`. Full lib 3724/0. Also added
`POST /api/geometry/cone` (commit ca7a684) — frustum + placement, which the
generic endpoint lacked — to build the nozzle. See `cone-rim-seam-alignment.md`.

### BOOL determinism 🔴 rbox-diag45 Intersection non-deterministic (10th digit)
`boolean_pipeline_determinism_gate` (boolean_fuzz_survey.rs) fails: a 45°-rotated
box Intersection yields bit-different volumes across two identical runs (~1e-10,
e.g. 2.7246017656 vs ...657). PRE-EXISTING (confirmed via `git stash` on the
clean tree 2026-06-15 — not the cone rim-seam fix). Same family as #34/#80
rotated/degenerate box over-inclusion. Likely HashMap/float-accumulation order in
the rotated-box marching-intersection path. NOT `#[ignore]`'d (reds the poke run
honestly). Task #61. Fix = deterministic iteration/accumulation, or pin honestly
if a true marching limitation.

### REVOLVE-TESS #63 🟢 FIXED — cone/sloped revolve bands non-watertight (commit 27d053c + fa26c18)
`revolve_profile` made a valid B-Rep but a NON-watertight MESH for any profile
with SLOPED (cone) bands: a band's two meridian arcs sit at different radii, so
the chord-driven edge cache sampled them with UNEQUAL counts, the structured
Coons-grid wedge declined (needs equal opposite counts), and the curved-CDT
fallback choked on the thin 3D sliver → the band emitted no triangles → holes
scaling with tessellation density (a revolved nozzle rendered as nothing). FIX
(tessellate_revolution_wedge): when opposite counts are unequal, triangulate the
wedge in its (u,v) PARAMETER square — well-conditioned regardless of radii — from
the EXACT boundary cache samples (watertight by construction). fa26c18: smooth
per-vertex surface normals on those wedges (flat per-band normal made sloped
bands render as faceted "rectangles"). Gate `tests/revolve_watertight.rs` (7
cases: tube/cone/frustum/stepped/engine/coarse+fine/partial-angle). Full lib
3724/0. Unblocked `POST /api/geometry/revolve`. LESSON: validate_solid_scoped
(B-Rep valid) ≠ watertight MESH — check manifold_report too. See memory
`revolve-tessellation-cone-bands.md`.

### SECTION #85c 🟢 FIXED — cutaway through a periodic seam dropped a generator (commit 398606d)
A section/cutaway was direction-dependent: a plane normal to +X gave the right
rectangle but a plane normal to +Y (which CONTAINS the cylinder's +X seam) gave
ZERO caps — the seam generator was reported at u≈−9e-13 (a hair below u_min=0) and
the UV-bbox trim's strict `>= u_min` dropped it. Fix: pad the trim's inside-test
to match the already-padded intersection search. Section now rotation-invariant
for axial planes. Gate `axial_cylinder_section_through_seam_85c`. KNOWN remaining:
oblique vertical planes (nz=0, off-seam) still 0 caps — separate pre-existing
marching-grid limitation.

### REVOLVE axis-touch 🔴 profiles with a pole (r=0) reject — TWO-PART FIX SPEC (2026-06-17)
**Investigated + scoped (experiment reverted to avoid shipping leaky domes):** the
fix is TWO parts that MUST land together:
1. **GUARD (face_intersects_axis, revolve.rs ~1547):** conditions (1) "vertex on
   axis" and (2) "edge sample r<tol" reject ANY touch of the axis. But a boundary
   vertex on the axis is a POLE and an edge lying ALONG the axis is the
   solid-of-revolution's axis segment — neither is a self-intersection. Only an
   edge CROSSING the axis (radial-offset sign-flip with both samples off-axis) or
   the axis piercing the face INTERIOR (condition 3, already skipped when the
   profile plane contains the axis) is real. RELAXING (1)+(2) to drop the
   touch-rejects while keeping the guarded sign-flip ADMITS the dome (verified:
   revolve no longer returns SelfIntersection). NOTE `make_on_axis_rectangle` +
   `face_intersects_axis_on_axis_rectangle_does_intersect` encode the BUGGY
   behavior — that rectangle (one edge on Z) revolves to a VALID cylinder, so that
   test must be updated when the guard is fixed.
2. **POLE TESSELLATION (the deep half):** with the guard relaxed, the dome builds
   a B-Rep-VALID solid but its mesh is NON-watertight — `open=147 nm=20` at the
   apex + a `cdt` panic (triangulate.rs, the #24 curved-CDT panic). The apex fan
   isn't closed watertight. This ties to #24 (curved-CDT spanning-triangle panic)
   and the pole-fan tessellation. UNTIL this lands, relaxing the guard alone makes
   domes build as silently-leaky solids (B-Rep valid, mesh open) — WORSE than the
   honest reject, so the guard relaxation was reverted. Ship both together.
   **LOCALIZED 2026-06-17:** the apex band is a `SurfaceOfRevolution` face that
   routes through `tessellation/surface.rs::tessellate_revolution_wedge` (the
   `"SurfaceOfRevolution"` arm, surface.rs ~207) — the structured-grid wedge
   declines for the degenerate apex band (one meridian collapses to the pole,
   r=0, so opposite boundaries have unequal sample counts), and the curved-CDT
   fallback chokes on the apex (all boundary points converge → degenerate contour
   → cdt panic → caught → empty → the 147 open). FIX LANE: in
   `tessellate_revolution_wedge`, detect the apex band (one boundary radius ≈ 0)
   and triangulate it as a FAN from the single apex vertex to the opposite rim
   samples (the analogue of the sphere-pole fan that already works for the sphere
   primitive) — never feed the degenerate apex contour to cdt. Then the guard
   relaxation (part 1) + this make eval_revolved_dome watertight together.
   **STRUCTURE PINNED 2026-06-17** (reproduced w/ guard relaxed, dumped the dome
   faces): every apex face is a **3-EDGE** SurfaceOfRevolution wedge —
   [r0..40][r40..40][r0..40] = two meridians (apex r≈0 → rim r40) + one rim arc
   (r40). But tessellate_revolution_wedge hard-returns false unless the loop has
   exactly 4 edges (surface.rs ~3845), so every apex wedge is rejected → curved-
   CDT → degenerate apex → 147 open. EXACT FIX: add a 3-edge branch to
   tessellate_revolution_wedge — find the apex corner (shared vertex of the two
   meridian chains, r≈0) and fan-triangulate the ring from it to the rim-arc
   samples (boundary-only, reusing cache samples → watertight); the 4-edge Coons
   path is untouched. CAVEAT: validate the repro profile yields a correct dome
   (the pie-slice try_dome decomposes into apex fans — confirm hemisphere not cone
   before un-ignoring the gate).
Repro `agent_build_eval::eval_revolved_dome` (#[ignore], asserts the desired
sound+watertight end state). (Original report below.)

### (orig) REVOLVE axis-touch — profiles with a pole (r=0) reject or go non-watertight
A revolve profile that TOUCHES the axis (a hemispherical dome apex, a solid cone
tip, a sphere's poles) is either rejected (`SelfIntersection`) or tessellates
non-watertight (sphere-via-revolve = 64 open at the poles). Blocks a whole class
of common solids of revolution with a pole (spheres, domes, ogives, solid cones).
Workaround: a small pole bore (vent) avoids the axis and is watertight. Found
2026-06-15 building a domed pressure vessel. Fix = handle the single-apex pole
case in revolve (apex vertex already has code; the self-intersection guard +
pole-fan tessellation need to admit it).
**PINNED 2026-06-17:** `agent_build_eval.rs::eval_revolved_dome` (#[ignore]'d) is
a forward-looking repro — a hemispherical dome (apex on axis). OBSERVED: the
revolve REJECTS with `SelfIntersection` (it never reaches tessellation), because
the profile's implicit closing edge runs ALONG the axis (both endpoints r=0). So
the dominant failure today is REJECTION, not the non-watertight-poles variant.
Un-ignore when the pole case lands.

### #65 🟢 FIXED — box∪cylinder boss mesh non-manifold at fine density (doubled facet)
Building an engine mount bracket: base plate (box) ∪ raised cylindrical boss came
back watertight=False open=12 (B-Rep valid), and chained difference of bolt-circle
/center-bore cylinders cascaded to open=1012→3932. The SAME bracket built clean
via the sketch-region path (rectangle + 13 holes in ONE extrude, no booleans).
So: the box∪cylinder coincident-face boss union leaves ~12 open (a #32/#84-family
residual on this config) and chained bores on an already-open solid compound it.
Task #65. LESSON for the agent/MCP: prefer sketch-region extrude for plates-with-
holes over boolean subtraction.

**RE-CHARACTERISED 2026-06-16 (interpenetrating boss, NOT coincident): the B-Rep
is SOUND, but the FINE mesh has REAL, density-dependent seam T-junctions.** Repro:
plate 120×80×16 ∪ coaxial analytic cylinder boss (r26 h45, INTERPENETRATING —
bottom buried inside the plate, no coincident face). Gate
`tests/tess_seam_tjunction_65.rs`.
- `validate_solid_scoped` (B-Rep) → **valid=true** (mesh-independent, SOUND). The
  part is NOT broken; the eye must judge on this (see EYE-SOUND, fixed bd426cd).
- `manifold_report` swept by chord on the SAME solid:
  `0.01 → nm=0 · 0.005 → nm=5 · 0.001 → nm=2`. The display/export default chord
  (`TessellationParams::default` = **0.001**) lands in the broken regime.
- **The weld is NOT the cause** — `manifold_report` (weld 1e-6) and the render's
  `1e-5` grid weld give IDENTICAL counts at every chord. Earlier "display-only
  artifact" framing was wrong: it is a real density-dependent mesh defect that
  also affects STL export.
ROOT: at the boolean pierce seam the cylinder LATERAL (curved) and the adjacent
plate-top PLANAR face (annulus with the circular hole) sample the SHARED seam
circle at DIFFERENT parameter points, so the two face meshes don't share seam
vertices → T-junctions. #58 fixed the developable-lateral over-refinement (the
flange case #84/#51) but NOT this curved↔planar shared-edge sampling at fine
density. FIX LANE = consistent shared boundary-edge sampling across adjacent
faces of different kinds at a boolean seam (#21 — which was closed for the cases
it covered but NOT this one — and #24). NOTE the original "open=12" reading was the
COINCIDENT-face boss (#32 family); the interpenetrating boss is open=0/valid.
LESSON: the diagnostic-render mesh is not a sound VALIDITY oracle (use B-Rep), AND
"watertight at chord 0.5" does not imply watertight at export density — sweep chords.

**LOCALIZED 2026-06-16 (the defect is NOT the boolean seam, and NOT plain
curved-CDT):** at chord 0.001 the non-manifold edges sit at **r=26.000, z≈34.583,
θ≈155°** — UP ON THE CYLINDER LATERAL, far above the plate seam (z=16), as `x4`
(four triangles on one welded edge = coincident edge pairs, not a classic
1+2 T-junction). Two further isolations: (a) a PLAIN cylinder (r26 h45 and r10
h20) is manifold at every chord 0.01→0.001, so the bare curved-CDT is fine; (b)
the defect only appears on the BOOLEAN-RESULT lateral (the piece above the plate,
whose boundary loop = the seam circle at z=16 + the top rim at z=41 + the vertical
seam, with cache-sampled boundaries). So the root is the curved-CDT triangulating
a BOOLEAN-MODIFIED cylinder lateral at high sample density — a degenerate/duplicate
near one interior spot — NOT the shared seam-edge sampling (that part is correct).
Deep #24-cluster (curved-CDT robustness on boolean-result faces). Candidate fixes
for the loop: (i) a final T-junction/duplicate-triangle removal pass over the
assembled solid mesh (safe, post-hoc), or (ii) an adaptive default chord (0.001
ABSOLUTE over-tessellates a 120 mm part ~120000:1 — also the source of the
build-time "jitter/hang"; a size-relative chord keeps normal parts out of this
regime AND speeds tessellation), or (iii) fix the curved-CDT degeneracy directly.
**FIXED 2026-06-16 — doubled-facet removal.** The exact triangles: `tri7055
[3925,3967,3944]` and `tri7056 [3967,3925,3944]` — the SAME three welded vertices
with OPPOSITE winding (a degenerate "fin", area ~0.002), so every fin edge
bordered 4 triangles → non-manifold. The curved-CDT emitted the sliver twice at
high density. The bare curved-CDT is fine (plain cylinder manifold at all chords)
and the shared seam-edge sampling is correct — it was purely a duplicate-facet
artifact. FIX: `weld_mesh_watertight_range` (the per-shell weld already in the
tessellate path) now, after the degenerate-collapse pass, CANCELS opposite-winding
facet pairs (drops both — the real tiling still covers the patch) and dedups
same-winding duplicates to one. No-op on a clean mesh (every facet's vertex-triple
is unique), so watertight primitives are bit-unchanged. Gate
`tests/tess_seam_tjunction_65.rs` both tests LIVE (B-Rep sound + fine-mesh
watertight at chord 0.001). No regression: revolve_watertight 7, primitive_tess_
watertight, revolve_analytic_faces 4, closed_edge_bore_rim_blends 5.

### EYE-SOUND 🟡 the agent-eye verdict judged the DISPLAY MESH, not the B-Rep
The MCP `verify_part` + auto-`perceive()` computed `watertight = (open==0 ∧ nm==0)`
from `GET /render?mode=diagnostic` — the DISPLAY tessellation. As #65 (re-char.)
shows, that mesh over-reports tessellation T-junctions, so the eye reported BROKEN
on geometry the B-Rep validator + `/perception` both call SOUND. A verifier that
false-alarms is worse than none — it made the agent build blind past a clean step.
FIX LANE: the eye's verdict must come from the SOUND channel
(`GET /api/agent/parts/{id}/perception`: `valid` B-Rep + manifold_report), with the
display open/nm demoted to a "mesh/display-quality" note. Backend `/perception`
already exists and reports correctly; the fix is to route the MCP eye through it.

### #33 🟢 Offset/partial-overlap chained union invalid
Fixed (line-extent classification).

### #34 🟢 Difference ops leave OPEN faces (counterbore floor)
Fixed via `drop_nested_inner_loops` after merge.

---

### #40 🟢 Faceted-cutter difference against a curved fillet face hard-failed
Found live by build-and-look: a filleted block minus a faceted bore that
overlaps a fillet → `"Invalid surface types for plane-cylinder intersection"`.
Two-layer cause: (1) the SSI dispatch classifies a flat RuledSurface cutter
wall as Planar, but `plane_cylinder_intersection`'s inner guard demanded
strict `surface_type()==Plane` and rejected it; (2) the fillet face is
cylinder-SHAPED but not the concrete analytic `Cylinder` (it's a NURBS/blend
surface), so even a corrected guard can't extract axis/radius. Fixed: the
plane∩{cylinder,sphere,cone} routines now identify the analytic operand by
DOWNCAST (so any planar surface is accepted as the plane) and, when the
curved operand isn't the concrete analytic type, FALL BACK to the marching
solver instead of erroring. The boolean no longer hard-fails on
cutter-meets-curved-face. Regression:
`boolean::tests::diff_faceted_cutter_against_fillet_cylinder_40`.
NOTE: this removes the hard error; full analytic correctness for
cylinder-shaped-NURBS faces (vs marching) remains part of the analytic-SSI
lane (#7) — and #24 (make extrudes/fillets emit concrete analytic surfaces)
would let the fast analytic path apply.

## Blends (fillet / chamfer)

### #82 🔴 Multi-edge ring chamfer — corner-patch not implemented
Chamfering a closed ring of edges errors at the shared corner vertices
(Cliff / ConvexCorner): "corner-patch synthesis for this vertex kind is
not yet implemented." Workaround: apply each edge in a separate call
(single-edge chamfer works on clean geometry).

### #37 🟢 Chamfer on a boolean-reshaped face → invalid B-Rep
NOT a chamfer bug — the *validator* was wrong. Any solid with a face that
has a hole (a through-bore, counterbore, or a box pierced by another box)
has `V−E+F ≠ 2`, because the naive Euler formula only holds when every
face is a disk. The validator used `V−E+F = 2`, so it rejected every
boolean result with a face-hole, which then blocked chamfer/fillet on it.
Fixed: `validate_euler_characteristic_for_solid` now uses the generalized
**Euler–Poincaré** identity `V − E + F − R = 2(S − G)` (R = inner loops,
S = shells, G = genus), counting R/S across all shells. Pierced/bored
solids validate; chamfer on a union result succeeds. Regression:
`boolean::tests::pierced_face_union_passes_euler_poincare_validation_37`.
The mixed-kind-corner cap's error filter was broadened to the `"Invalid
Euler"` prefix to keep matching the reworded message.

### #70 🔴 Chamfer crossing a fillet → self-overlapping solid
Chamfering over a previously filleted region produces a
topologically-clean but geometrically self-overlapping solid. Pinned.

### #38 🟢 Fillet rejected a geometrically valid radius
`validate_fillet_parameters` bounded the radius by `edge_length * 0.5` —
the WRONG dimension. A fillet's rolling ball runs *along* the edge; its
radius is limited by the *perpendicular room on the adjacent faces*, not
the filleted edge's own length. So R20 on a 30mm edge between 200/120mm
faces was falsely rejected. Fixed: bound by the shortest NEIGHBOURING
edge (the edges meeting this one at its endpoints — the perpendicular-room
proxy the tangent line runs along); isolated edges (no neighbours) defer to
downstream construction. Verified empirically: the construction was never
the limit — R20 on a 30mm slab edge constructs a watertight solid.
Regression: `fillet::tests::fillet_large_radius_on_short_edge_between_large_faces_38`;
the deliberate half-edge contract in `tests/fillet_radius_validation.rs`
was rewritten to the neighbour bound. 682 operations tests green.

---

## Queries / bounding box / mass

### #42 🟢 bbox/OBB/centroid of analytic curved solids ignored surface extent
Found live (ladder step 7). An analytic cylinder at center (0,0,0) r10 h40
reports `world_bbox min=(10,0,0) max=(10,0,40)` — ZERO extent in X/Y,
collapsed to the seam line at (r,0). The cylinder is V2/E3/F3 (2 seam
vertices); bbox/OBB/centroid iterate VERTICES only and never bound the
curved face's radial extent. Knock-on: OBB center (10,0,20) not (0,0,20);
assembly placement read every cylinder +r off in X. Geometry is correct
(transform moves the real solid; renders watertight) — only the QUERY lies.
Affects OBB, world_bbox, camera auto-frame, mass-properties centroid/inertia,
part_distance — for ALL curved primitives (cylinder/sphere/cone/torus).
Fixed: `solid_world_bbox` and `oriented_bbox_for` now bound the TESSELLATED
mesh (which samples every curved face's full extent), with the B-Rep
vertex hull as fallback for degenerate/empty tessellation. The OBB centre
now sits on the true COM instead of the seam. Regression:
`topology_builder::…::solid_world_bbox_captures_cylinder_radial_extent_42`
(cylinder r10 h40 → AABB x[-10,10] y[-10,10] z[0,40], centre (0,0,20)).
Suites green: readable 60, topology_builder 67, mass-inertia harnesses.

## Rocket-engine live build (2026-06-17) — two limits surfaced

### UNION-INTO-HOLLOW-REVOLVE 🔴 union of a solid into a hollow revolved body → non-manifold
Building the thrust chamber: unioning a solid cylinder (injector flange) into the
hollow revolved chamber failed — coincident-face base → 100 open; interpenetrating
→ 300 non-manifold + volume LOSS. The union of a solid that spans the chamber's
CAVITY + curved hollow wall (14-face revolved body: inner wall + outer wall +
bands + caps) overwhelms the boolean. WORKAROUND THAT WORKS: build the flange INTO
the revolve meridian profile (a wider rim band) — one watertight revolve, no
union — then bore the bolt circle (6× Ø8 chained differences ALL clean,
watertight, sound). So axisymmetric features → put them in the profile; bores →
fine; but discrete ribs / side mounting lugs (non-axisymmetric unions onto the
hollow body) hit this limit. Fix lane = boolean robustness for union vs a
multi-face hollow revolved operand (#32/#84 corefinement family). Repro: revolve
a hollow nozzle profile, create_cylinder flange, union → 300 nm.

### SECTION-DROPS-REVOLVE-BANDS 🔴 section_view of a revolved hollow part captures only planar/cylindrical faces
An axial section (plane ∋ axis) of the hollow thrust chamber returned only the
FLANGE cross-section (extent_v=22mm of a 200mm part, area 176) — the curved
SurfaceOfRevolution bands (bell, throat, chamber taper) were dropped, so the
de Laval meridian never shows. The engine itself is sound (watertight, valid,
render correct); this is a section/SSI gap on SurfaceOfRevolution faces (the
section marcher handles Plane/Cylinder but not the revolution bands). Ties to the
section lane (#85/#9). Fix lane = SSI arm for SurfaceOfRevolution in the section
clip. Repro: revolve a hollow profile, section_view normal [0,1,0] → only the
flange band appears.

## Mass properties / volume / verification honesty (2026-06-17 live dogfood)

### MASS-PROPS-⅓ 🟢 FIXED (integrator/endpoint) — /properties routed to mesh-based Tonon
The `/api/geometry/{id}/properties` endpoint (kernel_state.rs) used
`Solid::compute_mass_properties`, whose per-face divergence
`centroid·normal·area/3` drops the curved lateral flux (the lateral's surface
centroid sits on the axis, ⟂ its radial normal) → a cylinder came back at ⅓·πr²h
with a box-approximated/NEGATIVE inertia and area-weighted COM. FIX (commit on
this branch): route the endpoint to `BRepModel::mass_properties_for` →
`compute_solid_mass_properties` → `mesh_based_mass_properties` (Tonon
signed-tetrahedron, EXACT for curved faces). VERIFIED LIVE: an ANALYTIC cylinder
(`POST /api/geometry/cylinder`, r12 h26) now reports volume 11760 (≈πr²h),
COM [0,0,13], POSITIVE inertia. Gate `agent_build_eval::cylinder_mass_properties_are_correct`.

### EXTRUDE-CYL-MESH-INVERTED 🟢 FIXED 2026-06-17 — outward target was at the loop sample, not the surface sample
ROOT: `create_side_face_shared` (extrude.rs) passed `orient_face_for_outward` an
`outward_target` computed at the LOOP edge-midpoint, while
`orient_face_for_outward` reads the surface normal at the SURFACE parametric
midpoint. For a closed-circle lateral the extruded Cylinder's seam (ref_dir) sits
~90° from the loop's circle parameterisation, so the normal (at surface u=π →
angle π/2) and the target (at loop angle π) came out PERPENDICULAR → the dot was
~0 and the orientation fell to the wrong side → lateral wound INWARD → ⅓ volume,
COM at origin, NEGATIVE inertia (caps were fine). FIX: derive the outward target
from the SURFACE's own sample point — `create_side_face_shared` now takes the loop
centroid + inner_sign and computes `target = (surface_midpoint − centroid)·
inner_sign`, co-located with the orientation normal (the axial component is ⟂ the
radial lateral normal, so it doesn't bias the sign). RESULT: extrude-circle
cylinder → vol 11760, COM [0,0,13], POSITIVE inertia; lateral orient=Forward,
n·radial_out=+1. Gate `extrude_circle_cylinder_mass_props_correct` (un-ignored,
PASSES). NO regression: extrude lib 35, agent_build_eval 10, revolve_watertight 7,
primitive_tess_watertight, tess_seam_65 2, closed_edge_bore_rim 5, drawing 36,
bored_plate_caps. LIKELY also fixes the #41b extrude-path boss-wall drop (the
extrude boss lateral was inverted) — re-verify live after MCP rebuild. (Original
report below.)

### (orig) EXTRUDE-CYL-MESH-INVERTED — the sketch-extruded circle cylinder has an inverted mesh
SEPARATE, deeper bug surfaced by the fix above. Through the SAME (now-correct)
`/properties` endpoint: an analytic cylinder → 11760 (right), but an EXTRUDE-path
cylinder (MCP `create_cylinder` = `/api/sketch` circle + `/api/sketch/{id}/extrude`,
which `extrude_profile` turns into an analytic `Cylinder` via
`try_build_cylinder_from_circles`) → **3920 (⅓) with NEGATIVE inertia diagonal**.
Negative inertia is impossible for a consistently-outward mesh, so the
extrude-built Cylinder's LATERAL face is wound/oriented INWARD — its tessellated
mesh is inside-out on the lateral. This silently corrupts volume/mass for EVERY
MCP-created cylinder (the user creates cylinders this way), and likely feeds the
#41b extrude-path boss-wall drop. The create-response `perception.volume` (assembled
in the MCP) shows the same 3920. FIX LANE: orient the lateral face outward in
`extrude.rs::try_build_cylinder_from_circles` (match `create_cylinder_3d`'s
convention) — OR, pragmatic interim, switch MCP `create_cylinder` to the analytic
`POST /api/geometry/cylinder` (correct in every way: mass, mesh, booleans #41b).
Repro: build a cylinder both ways, compare `mass_properties_for` (analytic=πr²h+,
extrude=⅓+negative).
**NARROWED 2026-06-17 + KERNEL REPRO PINNED** (`agent_build_eval::
extrude_circle_cylinder_mass_props_correct`, #[ignore], FAILS): extruding a full
Circle profile (`create_face_from_profile_with_plane` + `extrude_face`) gives a
Cylinder whose SURFACE is byte-identical to `create_cylinder_3d`'s, and the CAPS
tessellate correctly, but the closed-circle LATERAL winds INWARD: mass-integration
= top-cap flux (3920) − lateral flux (7840) = −3920 → |·| = ⅓·πr²h, COM at origin,
inertia ⟂-diagonal NEGATIVE. So the surface/axis and `orient_face_for_outward`'s
chosen FaceOrientation are NOT the culprit (the B-Rep face is outward) — the fault
is the curved-CDT tessellation winding of the extrude's QUAD side-loop
(`create_side_face_shared`: bottom-circle-fwd → vertical → top-circle-rev →
vertical) vs `create_cylinder_3d`'s native lateral loop, with the same surface +
outward orientation producing opposite triangle winding on the closed-circle
seam. FIX LANE: make the curved-CDT honor the face's outward orientation
independent of the side-loop's 2D winding (or align the extrude side-loop winding
with the primitive's). DEEP — extrude path is load-bearing; fix with fresh focus.


Surfaced rebuilding + driving a bored plate through the LIVE api-server (the
verification-layer dogfood Varun asked for). THREE intertwined findings; the
B-Rep boolean itself is INNOCENT.

### TESS-ANNULAR-CAP 🟢 FIXED 2026-06-17 — annulus_radial_strip mis-classified a square cap as a ring
**ROOT + FIX.** `annulus_radial_strip`'s `circular()` ring-detector accepted the
bored-plate cap's OUTER loop (a square sampled at its 4 corners) as a circle —
because the 4 corners are equidistant from the centroid (40√2) — then radial-
stripped the "ring" to the bore (n+m = 4+300 = 304 triangles), over-covering the
annular cap to area 8320 (vs 5948) and inflating the bored solid's mesh volume to
107817 (vs 95162). The B-Rep, the boolean, the mass-props integrators, and
`triangulate_planar_polygon` were ALL innocent (a synthetic square+circle through
the general CDT was always correct). FIX: a chord<radius guard in `circular` —
a genuinely circular tessellated ring has every consecutive chord below its
radius (2r·sin(π/n) < r for n ≥ 7), but a 4-corner square's side (80) exceeds its
corner-radius (56.57), so the square now falls through to the general CDT (which
triangulates square-outer + circular-hole correctly). RESULTS: bored cap area
8320 → 5947.6; bored-plate mesh volume 107817 → 95165; the bored cylinder
`analytic_cylinder_bore_is_smooth_and_watertight_24` flipped RED → GREEN as
collateral. Gates: `tessellation::surface::tests::bored_plate_caps_tessellate_to_annulus`
+ `planar_face_square_with_circular_hole` (lib) + `agent_build_eval::
bored_plate_mesh_volume_correct` (un-ignored). NO regression: revolve_watertight
7/7 (washer annuli use dense circular rings, still radial-stripped),
primitive_tess_watertight, tess_seam_tjunction_65, closed_edge_bore_rim_blends,
drawing 36, boolean 99/3 (the 3 are pre-existing #27 chained-union, unrelated).
LESSON: no test checked face AREA / solid VOLUME, so a watertight-but-wrong mesh
hid for a long time — VERIFY-EFFECT (volume/area gates) is the durable guard.

(historical) PINPOINTED 2026-06-17. The bored plate's wrong volume + filled-looking hole is
NOT triangle inversion and NOT the boolean — it is the ANNULAR PLANAR CAP
triangulating with OVERLAPPING triangles. Per-face measurement
(`diag_bored_plate_face_winding`): the top/bottom caps (a Plane face whose inner
loop is the Ø24 bore) tessellate to **area 8320 each — larger than the cap's own
80×80 outer square (6400)**, which is only possible if triangles overlap; the
correct annulus is 80²−π·12² = 5948. That excess area maps EXACTLY to the volume
error: each cap contributes ⅓·8·8320 = 22187 instead of ⅓·8·5948 = 15861, and the
+6326 × 2 caps = +12652 ⇒ 95162 + 12652 = 107814 ≈ the observed 107817. The bore
WALL is correctly oriented (points toward axis, z∈[−8,8]); the integrators are
fine. So the fix is in `tessellation/surface.rs::triangulate_planar_polygon` (the
`cdt::triangulate_contours` hole path) / the cap contour construction for
boolean-result faces — the hole is not being erased, so the cap is covered twice.
This is LOAD-BEARING (every plate-with-hole uses it) — fix carefully, gate on cap
AREA == analytic (no current test checks face area, which is why it hid).
Pins: `diag_bored_plate_face_winding` (#[ignore], localises per-face area),
`bored_plate_mesh_volume_wrong` (#[ignore], FAILS on the volume).

PRIOR (superseded) framing — kept for the signed-tet evidence that exonerates the
integrators and boolean:
- a `create_cylinder_3d` cylinder integrates CORRECTLY at both default and fine
  tess (11754 / 11760 ≈ 11762, watertight). So the Tonon signed-tet integrator
  (`mesh_based_mass_properties`) AND `mesh_analytics` (the eye) are both correct,
  and the kernel-primitive cylinder mesh is well-oriented.
- the kernel bored plate (`create_box_3d − create_cylinder_3d`) integrates to
  107817 by BOTH integrators identically (signed-tet == mesh_analytics), while
  watertight (open=0 nm=0). A watertight mesh enclosing MORE than the un-bored
  solid block (102400) is only possible if some triangles are wound INWARD — here
  the bore wall (the bore is a real B-Rep void, ray-parity confirms; the wall IS
  meshed, ~1400 verts at r≈12 — `kernel_bored_plate_mesh_has_bore` PASSES). So the
  bore-wall (and/or annular-cap) triangles are emitted with reversed winding.
- the LIVE/MCP (extrude-path) cylinder shows the SAME disease from the other end:
  `mass_properties` returns volume ⅓ (3920) AND NEGATIVE inertia diagonals — a
  consistently-outward mesh can NEVER yield negative inertia, so its mesh also has
  inverted triangles.
USER-VISIBLE EFFECT: the inward bore wall renders as a filled cap (ids-top = solid
face, no annulus; depth uniform) → "the subtraction didn't work" in the viewport,
and STL export would be wrong. Pins: `bored_plate_mesh_volume_wrong` (#[ignore],
FAILS — kernel repro), `diag_cylinder_mesh_orientation` (#[ignore], characterises
both), `kernel_bored_plate_mesh_has_bore` (PASSES — B-Rep + wall present, so the
fix is in TESSELLATION winding, NOT the boolean and NOT the integrators). FIX
LANE: orient triangulated triangles outward per face on boolean-result + extrude
solids — the per-face winding should follow the analytic outward normal (the bore
wall's outward points toward the axis = into the void; cap inner-loop winding must
oppose the outer loop). Likely the same place that sets face/triangle winding from
`FaceOrientation` for split/new faces. Possibly related to #65/#21.

### VERIFY-EFFECT 🔴 the verification layer false-greens "no-effect" / wrong-volume ops
Both bugs above sailed through EVERY check: `perception` reported
`watertight:true, valid:true, sound:true, verdict:"OK — closed manifold solid"`
on a plate with no visible hole and an impossible volume. ROOT: the verdict
proves VALIDITY (closed manifold + B-Rep valid) but NOT the operation's intended
EFFECT or physical sanity. A solid plate is also a valid watertight solid, so a
difference that removes nothing still reads "OK". The agent_build_eval harness
shared the blind spot — it asserted watertight + dims (unchanged by a bore) +
bore-face-exists (the wall face exists even when mis-meshed), never volume. FIX
LANE (the real "verify the verification layer"): add EFFECT + physical-sanity
gates — a Difference must DECREASE volume; a Union must not decrease it; volume ≤
bbox-volume·(1+ε); volume > 0; COM ∈ bbox; inertia positive-definite. The
integrators are trustworthy TODAY (signed-tet is exact on a well-oriented mesh),
so this gate would have CAUGHT TESS-ORIENT immediately — it is the right next
guard once the mesh winding is fixed (TESS-ORIENT). LESSON: "sound/watertight" was necessary but never
sufficient; the eye-agreement + bore-recognition added this session also pass on
this part (dims + a cylinder face both survive), so they too need a volume/effect
arm.

## Validation / model lifecycle

### #29 🟢 Op post-validation runs over the WHOLE model
A new operation's validation validated *every* solid, so any unrelated
invalid solid (e.g. a #36 husk) blocked an otherwise-valid op.
Added `validation::validate_solid_scoped(model, solid_id, …)` (keeps
errors on the touched solid + model-global, drops other solids') wired
into the 7 single-solid ops (chamfer, fillet, revolve, transform, draft,
loft, shell), and `validate_faces_scoped(model, &faces, …)` for the
face-set ops `blend` + `pattern` (derives owning solids from the faces;
#39). Guarded by `brep_validation_oracle::scoped_validation_ignores_unrelated_invalid_solid`.

### #24 🟢 Round features were faceted prisms (axial seam lines), not cylinders
A circular profile extruded to an N-gon prism → N planar wall faces with
visible axial seam lines, and inflated boolean facet counts. Resolved by
exposing the kernel's ANALYTIC cylinder as a build primitive:
`POST /api/geometry/cylinder {center,axis,radius,height}` →
`create_cylinder_3d` → one smooth periodic lateral face (V2/E3/F3, ~1 seam,
not 24 edges). Verified live: the cylinder primitive renders smooth (no
axial lines), and a block − analytic-cylinder bore is watertight (open=0/
nm=0) and smooth — the difference uses the analytic plane∩cylinder SSI path
(unblocked by #40). Regression:
`boolean::tests::analytic_cylinder_bore_is_smooth_and_watertight_24`.
NOTE: polygon-profile extrudes are still (correctly) faceted — a polygon IS
faceted; round features should use the cylinder primitive. A circle-edge
profile path through `extrude_profile` (which already emits a Cylinder from
a Circle edge) is a possible future convenience but not required.

### #28 🟢 FIXED — full 2π revolve of an offset rectangle (was tessellation_empty)
Resolved by REVOLVE-ROBUSTNESS #47 (per-segment wall wedges tessellated as
structured grids) + the tessellation fixes; verified by re-audit 2026-06-14.
`revolve_volume_invariants` (14 cases, partial + full 2π) all tessellate
non-empty with correct Pappus volume, and the harness now also asserts every
revolve is a valid B-Rep (`validate_solid_scoped`) + watertight
(`manifold_report`). Commit f3d0006.

---

## Drawing / dimensioning

### DRW-DIM-EXPLOSION 🟢 FIXED — dimension selection + circle de-clutter
**FIXED 2026-06-16.** `visible_dimensions` now runs `select_dimensions`: drop
per-band cone half-angles when there are several, collapse near-equal (kind,
value) callouts, cap diameters to the largest-3 + smallest-2 (envelope + throat),
and a hard per-view cap of 8 prioritising extents > diameters. `build_hlr_view`
runs `select_circles`: dedupe coincident circles and cap CONCENTRIC rings (same
centre) to largest-3 + smallest-2 while keeping scattered same-radius circles (a
bolt pattern). Bell-nozzle drawing went from dozens of overlapping ∠/Ø callouts +
~12 concentric circles to ≤8 dims/view + ~7 circles — readable. Drawing tests 36,
oracle 6, drawing_mgr 54 green. REMAINING (separate, not the explosion): tangent-
edge suppression (smooth band boundaries still draw as faint lines; the silhouette
isn't a B-Rep edge so it needs real silhouette handling) — original report below.

### (orig) auto-dimensioning + circle projection don't scale to complex revolves
Surfaced live building a rocket-engine bell nozzle (revolve, ~19-segment hollow
profile). The solid is SOUND (B-Rep valid, watertight). But its auto drawing is
UNUSABLE: `visible_dimensions` emits a callout for EVERY analytic band — a nozzle
has ~9 cone bands, so the FRONT view stacks ∠36.9°/∠30.3°/∠24.8°/… AND the bottom
floods with Ø150/Ø136/Ø112/Ø84/Ø72/Ø60/Ø44… extension lines, all overlapping.
The TOP view draws a concentric circle for EVERY band ring (a dozen dashed
circles) — the analytic-circle feature (true circles, good) amplifies the clutter.
ROOT: the dimensioning has no SELECTION — it annotates every feature instead of
the few that define the part (overall L + OD, throat Ø, exit Ø, chamber Ø). FIX
LANE: (1) a dimension-selection pass (dedupe near-equal values, keep extents +
distinct key diameters, cap per view); (2) circle de-clutter (only distinct-radius
rings, drop interior duplicates); (3) extend `drawing::verify` to FLAG
over-dimensioning / label-collision density so the oracle drives it (the user's
"better verification layer to improve"). The simple box / single-bore parts draw
clean (commit 7e59347); the gap is COMPLEX parts.

### REVOLVE-TESS-SEAM 🟡 MITIGATED — revolve band boundaries non-manifold at very fine chord
**MITIGATED 2026-06-16 by a size-relative chord floor in `tessellate_solid`.** An
absolute chord (the 0.001 mm default) is size-blind — 178000:1 on a 178 mm part —
which is what pushed adjacent bands into the non-conforming regime AND caused the
build jitter. `tessellate_solid` now floors the chord at `5e-4 · bbox-diagonal`
(chord can only get COARSER, never finer; coarse explicit chords like
manifold_report's 0.5 are untouched). The bell nozzle now tessellates MANIFOLD at
the default/export density (was nm=2 + a seam sliver). No regression:
revolve_watertight 7, primitive_tess_watertight, tess_seam_tjunction_65 2,
revolve_analytic_faces 4, closed_edge_bore_rim 5, fillet_closed_edge 5. NOTE this
SIDESTEPS the regime rather than fixing the underlying band-boundary shared-edge
sampling — a part forced to an absolute chord finer than the floor could still
hit it; the true fix (revolve bands share boundary samples at any density) stays
open under this heading. Original report below.

### (orig) revolve band boundaries non-manifold at very fine chord
Same nozzle: at the DISPLAY default chord (0.001 ABSOLUTE — 178000:1 on a 178 mm
part) the mesh has 2 non-manifold edges (x4 fans) at band boundaries (bell z≈164,
chamber z≈48) + a thin seam sliver (the visible "extrusion" in the shaded render).
Manifold at chord 0.05; the B-Rep is valid + watertight (sound eye correct). The
adjacent revolve bands don't share boundary samples at extreme density — the
#21/#24 shared-edge-sampling lane, on the REVOLVE seam (the boolean-seam sibling
#65 was fixed via doubled-facet removal, but these are x4 fans, not doubled
facets). Compounded by the absolute-0.001 default chord over-tessellating (also
the build-time jitter source) — a SIZE-RELATIVE default chord would sidestep this
regime AND speed display. Pin lane: revolve band-boundary shared sampling.

## Other notes (not bugs, but gotchas)

- **Edge IDs are global and accumulate across solids.** A "fresh box" is
  not always edges 0..11. Always probe `GET /api/agent/edges/{id}` and
  classify by endpoint coordinates before selecting edges for a blend.
