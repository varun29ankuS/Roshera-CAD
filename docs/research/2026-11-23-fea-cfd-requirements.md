# FEA / CFD and the Roshera mesher — requirements (draft for Varun)

Status: DRAFT, 2026-11-23. Nothing here is built. Decisions marked **(decided)**
are Varun's from the 11-23 discussion; everything else is a proposal to settle
before code.

## 1. What we are building and why

An agent runs a structural or flow analysis end to end, the kernel certifies
every step, and the engineer gets a result card plus a replayable record.
**(decided)** First slice proves one job each, on one shared foundation:

| Track | Customer shape | Job | Answer the card gives |
|---|---|---|---|
| FEA | Volvo-type bracket / housing | Is it strong and stiff enough? | safety factor, peak stress location, deflection, assumptions, evidence |
| CFD | Okulo aerofoil section | Lift, drag, moment, stall | C_L(α), C_D(C_L), C_m, stall angle vs published data, refusal where the model is not trusted |

**(decided)** Agent scope: analyse, then propose design changes on branches;
the engineer picks. **(decided)** Compute: local first, cloud later.
**(decided)** Engineer sees the card plus every step in the timeline.
**(decided)** Materials: built-in library with cited sources + customer overrides.

## 2. The mesher **(decided: our own, in Rust)**

Clean implementation. Open codes (Gmsh, Netgen, TetGen, MMG, CGAL,
cfMesh, snappyHexMesh, fTetWild) are read for ideas and run beside ours as
bench ORACLES only — never linked or copied (AGPL + a commercial licence
option need code we own).

### 2.1 Meshes each job needs

| Job | Element | Where it must be fine | Hard part |
|---|---|---|---|
| Bracket FEA | quadratic tetrahedra (10-node; CalculiX C3D10) | fillets and holes on the load path, contact/fixture faces | robust boundary recovery; quality (no slivers) |
| Aerofoil CFD, 2D | structured C-mesh (quads) round the section | leading edge, trailing edge, wall-normal first cell (y+ target) | far-field size, wake resolution, growth rate |
| Later: 3D CFD | hex/poly core + prism layers at walls | walls, wakes | layers that survive corners and gaps |

### 2.2 Approach: exact boundary, not an envelope

Robust tet meshers of the TetWild family accept dirty input by letting the
boundary deviate within an envelope; a 2026 TOG paper (Diazzi, Dai, Panozzo,
Attene, "Surface chamfering for robust tetrahedral meshing") notes this puts
geometric error over the whole boundary and cites a 2023 provably robust
Constrained Delaunay Tetrahedrization. Our input is NOT dirty: it is the
kernel's certified, watertight, face-tagged surface mesh, and the kernel has
exact predicates. So the proposal is boundary-conforming CDT with exact
predicates, then quality refinement — the boundary stays exactly the
certified surface, which keeps every certificate honest. (To verify against
the 2023 paper before design.)

### 2.3 The mesh is a first-class object the agent can query ("mesh lens")

Every element carries: the persistent face / feature it meshes (boundary
elements), its quality measures, WHY it has its size ("fillet on the load
path", "leading edge"), and after a solve its error estimate. The agent asks
"is the mesh trustworthy at the peak stress?" and gets evidence.

### 2.4 Certificates

- Boundary fidelity: every boundary element within a stated distance of the
  exact surface (the deflection gate already owed in `workplan-2026-11-22`).
- Quality: no element below the stated quality floor (per customer spec).
- Convergence: the reported quantity agrees between two meshes within a
  stated tolerance, or the result is refused.
- Coverage: every load / fixture face meshed and named.

## 3. Problem set-up by the agent

Verbs (MCP + REST, recorded in the timeline): `sim_study`, `sim_material`,
`sim_fixture`, `sim_load`, `sim_mesh`, `sim_run`, `sim_check`, `sim_card`.
Faces are picked by meaning (kernel feature recognition: "the four bolt
holes", "the base face"), held by persistent id.

**Pre-solve checks (the kernel refuses with a typed reason):** rigid-body
motion constrained (DOF accounting, as sketches); every load on a real face
with units and direction; material assigned; idealisations legal (a fillet on
the load path is not removed).

## 4. Feedback **(decided: all four)**

1. Pre-solve refusals (seconds) and post-solve evidence (minutes): two-mesh
   check, singularity detection (stress rising with refinement at a sharp
   corner), reaction balance.
2. Engineer corrections -> part-family playbooks.
3. Scored bench grows; held-out cases never tuned on.
4. Per-customer defaults (materials, safety factors, load conventions, mesh
   spec); plus visible agent self-review after each run.

## 5. Trust bench **(decided: all four layers)**

| Layer | First cases | Source |
|---|---|---|
| Hand calcs | cantilever tip deflection, plate with a hole (Kt ~ 3), thick cylinder | textbooks (cite edition when used) |
| Published | NACA 0012 C_L/C_D/C_p — NASA Turbulence Modeling Resource: seven SA codes agree within ~1% lift, ~4% drag | tmbwg.github.io/turbmodels |
| Their tool | pilot parts vs their ANSYS/Abaqus/STAR-CCM+ | pilots |
| Physical | pilot test rigs / tunnels | pilots |

Mesh bench beside it: zero failures on large CAD sets (ABC, Fusion 360
Gallery, CADGenBench), time to a certified answer, accuracy per DOF,
against the oracle meshers.

## 6. Solvers

FEA: CalculiX (current release 2.23 per calculix.de; C3D10 documented in the
2.22 manual §6.2.7) as a subprocess. CFD: OpenFOAM or SU2 as a subprocess
(to choose; both need checking). Solvers are external processes, not linked.

## 7. Proposed order of work

1. Aerofoil 2D structured C-mesh + its certificate (small, deterministic;
   oracle: published grids on the TMR site).
2. Surface mesh -> exact-boundary CDT tet mesh for single parts + quality
   refinement + mesh lens; oracle: TetGen / Netgen / fTetWild on the mesh
   bench.
3. CalculiX integration, pre-solve checks, two-mesh convergence, card.
4. Aerofoil CFD run (solver subprocess), α sweep, refusal near stall.
5. Feedback loops and pilot benches.

## 8. Open questions for Varun

1. Order: start with the aerofoil 2D mesh (fast, visible, Okulo) or the
   bracket tet mesh (harder, Volvo)?
2. CFD solver: OpenFOAM (broad, industry) or SU2 (aerodynamics, adjoint shape
   optimisation)?
3. Do pilots have a company mesh specification we can ask for now?
4. Material library: which standard / source first (to verify edition before
   citing)?
