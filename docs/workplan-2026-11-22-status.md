# 90-day workplan — status against `roshera-drawing-dfm-plan.pdf`

Checked 2026-11-22 against main at 8be09cc1. Corrections to the plan's
numbers and scope, from work that landed the same day.

## Workstream A — drawings

| Plan item | Status | Change to the plan |
|---|---|---|
| P0 kill the stall on 700+ face parts | Part 109: 446 s → **287 s**. Shared UV-loop / membership caches now span section, adjacency, visibility and the certificate (4e4637a7, 9549a9d5, 1985ca51, 10dadca5). | The UV-footpoint cause is fixed. The remaining cost is the hidden-line pass (**HLR pass 2 ≈ 153 s**). P0 now targets HLR. |
| P0 15-part pilot benchmark | 13 parts scripted (`roshera-eval/drawing-bench/bench.py --only …`), 0 layout errors, all sound. No Volvo / wing-rib / drone parts yet. | Needs an **answer key** per part (expected dimensions, expected DFM failures) so "0 false passes" is measurable; needs the real pilot STEP files (ask in week 1, with Volvo's drawing standard and a sample drawing). |
| P1 datum-first dims | Detail views (16c7f519) locate crowded features by **chains** from a datum-located anchor. | Conflicts with "baseline, not chained". Convert detail chains to baseline / ordinate (ISO 129-1) — ordinate.rs is built, unwired. |
| — | Sheet orientation (7eaae786): portrait/landscape, search tries both. | Not in the plan; small, done. |

## Workstream B — CNC DFM

Shipped as `dfm/packs/cnc.rs` (`cnc_3_axis`, 8be09cc1), not `machining.rs`.

| Plan rule | Shipped | Gap |
|---|---|---|
| cnc.sharp_internal_corner + cnc.min_internal_radius | `cnc.internal_corner_radius`, `cnc.corner_reach` (depth / 2r ≤ 4) | plan's 6:1 depth/radius differs — pick one with a sourced default |
| cnc.deep_bore | `cnc.hole_depth_ratio` (≤ 10), `cnc.standard_drill_size`, `cnc.blind_hole_floor`, `cnc.min_hole_diameter` | warn tier at 4×D, fail at 8×D |
| cnc.min_wall | `cnc.min_wall` (0.8 mm) | plastics/composites 1.5 mm |
| cnc.internal_void | only in casting / molding packs | add to CNC |
| cnc.tool_access_setups | `cnc.hole_access` (hole axes vs setups only) | **all faces along ±X ±Y ±Z, minimum setups, undercuts** — the fixture-cost rule |
| P1 face tints | `DfmFaceTints.tsx` exists; violation animations shipped | DFM in the evidence pack (PDF appendix + JSON) |
| P2 sheet metal | shipped (6 rules) | frozen; see PARKING_LOT.md |

Citations: no threshold is claimed from a standard; defaults are shop
practice with vendor-guide URLs (`docs/dfm-plan-2026-11-22.md`). A
*Machinery's Handbook* citation enters only after checking the edition
and table.

## P0 import speed: the size-relative edge cap is parked on a mirror defect

Part 109's import certificate meshes 1.17 M triangles because the
tessellation edge-length cap is an absolute 0.1 mm: every long, thin
fillet is cut into 0.1 mm strips. Flooring the cap at 0.5 % of the part's
size (`docs/patches/2026-11-22-size-relative-edge-cap.patch`, with its
test) cuts that to 277 k triangles and the certificate mesh from ~22 s to
~9 s, and passes all 4634 geometry-engine lib tests. The full red-gate
finds ONE new red: `tests/mirror_orientation.rs`
`a_bore_rim_fillet_commutes_with_a_mirror` -- the fillet of the mirrored
bore rim is refused, geometrically open (394 boundary edges).

Root cause (found, not yet fixed): on the mirrored part the bore wall's
top rim (a closed `Arc`, edge 50, used reversed) samples half a turn away
from the seam vertex it closes on: the seam line meets the top at native
u = 4.712, the arc's own t = 0 / t = 1 point projects to u = 7.854. The
wall's outline therefore doubles back along the rim and the curved CDT
refuses it (`OutlineOverlapsItself`). The 0.1 mm cap hid it: both rims
hit the 100-segment ceiling and the two traversals welded into a harmless
slit. Fix the closed-arc start under mirror (`Arc::transformed_arc`,
orientation-reversing branch, and/or the edge range flip in
`operations::transform`), then land the patch.

## Calendar

Weeks 4–6 (CNC pack) are mostly done → pull live pilot parts forward
about two weeks.

## Carried in from older docs (2026-11-22 doc consolidation)

Open items found in retired docs that bear on "0 false passes" or on pilot
parts. Everything else from those docs is in `PARKING_LOT.md`.

| Item | Source | Why it is in scope |
|---|---|---|
| The certificate has no deflection (distance) gate | `roshera-eval/speed/FAST_EXACT_CERTIFICATE.md` §1.4, ranked item 6 (11-17); no landing commit, and no "deflection" in `geometry-engine/src/harness/` or `tessellation/` | A mesh that strays from the surface can certify sound: a false pass |
| Revolution `closest_point` assumes the profile lies in one meridian plane and returns a wrong answer as success | `docs/archive/DIAGNOSIS_217_211_249.md` 249-B; no landing commit found | Imported revolved faces feed sections and drawing dimensions |
| STEP-import soundness on real parts: CADGenBench fixes 3, 4, 6–9 open; robot arm 70 of 572 unsound; GrabCAD vice 11 of 13 unsound | same diagnosis; `docs/backlog-2026-11-06-kernel-trust.md` STEP-3, STEP-IMP-1 | A pilot STEP part that will not certify cannot get a drawing; take one only when a pilot part shows it |
| Drafting gaps from the 11-17 critique: view choice (drop views that add nothing), sections that carry internal dimensions, complete hole callouts (blind depth, cbore/csink on blind holes, threads as authored notes), general notes and TYP | `the 11-17 drawing critique (deleted 2026-11-22; open items listed here)` §6 items 3, 4, 8, 9; `BENCHMARK.md` "Not done" | Each can leave a pilot sheet incomplete; status not re-checked |
| Cross-bore breakout meshes with off-surface slivers (`max_normal_deviation_deg` 83.6) and the certificate reads the part unsound | `docs/archive/2026-08-22-agent-surface-open-work.md` §6; pinned RED `geometry-engine/tests/cross_bore_mesh_wings.rs:262` (`#[ignore]`, not listed in `KNOWN_REDS.md`) | A pilot part with cross-drilled holes would not certify; take it when a pilot part shows it |
| ISO 4762 counterbore table missing (agent refused to size counterbores) | `docs/campaign-2026-11-12-drawings-production.md`, "Gaps carried" | Already the plan's P1 standards-library task |

## Order now

1. P0: HLR pass on part 109.
2. P0: one-command 15-part benchmark with answer key.
3. CNC: full-face setup access + undercuts; internal void; tiers.
4. P1: detail-view chains → datum-first (ordinate).
