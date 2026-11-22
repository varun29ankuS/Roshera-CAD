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

## Calendar

Weeks 4–6 (CNC pack) are mostly done → pull live pilot parts forward
about two weeks.

## Order now

1. P0: HLR pass on part 109.
2. P0: one-command 15-part benchmark with answer key.
3. CNC: full-face setup access + undercuts; internal void; tiers.
4. P1: detail-view chains → datum-first (ordinate).
