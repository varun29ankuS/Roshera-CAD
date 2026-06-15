# Known Bugs — Roshera Kernel

A living ledger of defects found while exercising the kernel through the
API and tests. Append as we find; move to **Fixed** (with the commit)
when closed. The diagnostic render (`GET /api/agent/parts/{id}/render?mode=diagnostic`,
`open_edges`/`nonmanifold_edges`) is the standing oracle for geometry validity.

Status key: 🔴 open · 🟡 in progress · 🟢 fixed

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

### #1 🔴 Cone-radial conic-cut blindness (worst class, task #1, 21 HARD cells)
A z-axis cone shifted off-axis (base at x=1.0) so its slanted LATERAL surface
pierces a box side wall (plane x=1). The cone-lateral × plane intersection is a
CONIC — a HYPERBOLA when the cutting plane is parallel to the cone axis — not a
circle or a line. The split/classify/stitch pipeline cannot form that hyperbolic
boundary curve, so the conic patch is dropped or mis-stitched. Characterized
signature (`radial-face+x`: bc=[1,0,-0.5] rb=0.5 rt=0.3 h=1):
- **∩** → `InvalidBRep` "component 0 has only 3 planar faces" — the hyperbolic
  conic patch is dropped entirely, so no closed manifold can form.
- **∪** → vol −1.2%, open=6, nonmanifold=2, odd Euler (3 faces share the conic
  boundary edge + boundary gaps).
- **∖** → vol −1.3%, open=8, odd Euler (gaps along the conic cut).
**LOCALIZED (2026-06-15):** the analytic cone×plane SSI arm is CORRECT — the
new lib test `boolean::tests::cone_plane_ssi_points_lie_on_both_surfaces_1`
confirms every intersection-curve point lies on BOTH surfaces for the circle /
two-generator-line / hyperbola orientations. So the defect is DOWNSTREAM of the
SSI: the cone-LATERAL patch is dropped/mis-stitched in split_faces / classify
(∩ keeps only the 3 planar faces — base disc + top disc + +X wall). Note the
`radial-face+x` cell is the TWO-GENERATOR-LINE case (the +X wall plane contains
the cone axis), not a hyperbola — `plane_cone_parallel_intersection` already
emits those two lines correctly. Fix lane: make split/classify KEEP the cone
conic patch — DEEP, multi-fire, ties task #7. Pinned: `boolean_fuzz_survey.rs::
cone_radial_conic_cut_pin_1` (#[ignore], asserts watertight+valid+vol — flip on
when #1 lands). The 33-cell curved poke matrix (`harness::poke_matrix`) stays
fully green; this off-axis conic class is the remaining frontier.

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

## Other notes (not bugs, but gotchas)

- **Edge IDs are global and accumulate across solids.** A "fresh box" is
  not always edges 0..11. Always probe `GET /api/agent/edges/{id}` and
  classify by endpoint coordinates before selecting edges for a blend.
