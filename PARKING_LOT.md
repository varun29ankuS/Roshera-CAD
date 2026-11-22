# Parking lot (90-day workplan, 2026-11-22 → 2027-02-20)

Triage rule: *does this block a pilot part or make a pilot drawing/DFM
verdict wrong?* Yes → fix it. No → it goes here, unopened for 90 days.
Plan: `docs/roshera-drawing-dfm-plan.pdf`; status against it:
`docs/workplan-2026-11-22-status.md`.

## Parked by the plan
- 3D CAD feature expansion (sweeps, variable fillets, complex lofts)
- FEA / physics simulation and the meshing feedback loop
- Omnigraph / PLM work-graph trials
- AI-lab RL corpus (roshera-rl) and multi-tenant cloud
- Big refactors (boolean.rs / main.rs split)
- Website hero animations and shodh-* repos
- Generalist VC cold outreach (until the Volvo PO)
- NURBS authoring and NURBS×NURBS booleans (keep only STEP-import speed)
- Sketch ARCH-1..5, polyline solver (fix SK-H2 / SK-M2 only for a live demo)

## Parked from the work list (found after the plan)
- DFM sheet-metal pack beyond what shipped (8be09cc1): bend relief,
  K-factor flat pattern, hole-to-hole spacing — unless the drone supplier
  sends bent parts
- DFM casting pack beyond what shipped (8be09cc1)
- Drawing sheet orientation switch after creation (PATCH verb) and an app
  sheet selector (7eaae786 chooses orientation at creation only)
- Drawing lettering A0–A2, duplicate RIGHT view, characteristic numbers
- MCP KB `general_tolerance` still cites ISO 2768-2 (cancelled; ISO
  22081:2021) — fold into the P1 standards-library task, not before
- Vault KB text for CNC / sheet metal / casting packs (says "cannot run")
