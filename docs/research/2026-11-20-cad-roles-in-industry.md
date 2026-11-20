# How each worker uses a part — roles in industry

Research note, 2026-11-20. Companion to
`2026-11-20-cad-workflow-variations.md` (the workflows); this one is the
PEOPLE. For each role: what they do, what they receive, what they hand on,
what they check, what "done" means to them, what a checkpoint means to them,
and where Roshera stands. The list is deliberately wide; the same part passes
through many of these hands.

Legend for "Roshera fit": ● serves it today · ◐ ingredients exist ·
○ not built.

---

## A. Design and engineering

| Role | Does | Receives → hands on | Checks / "done" means | A checkpoint is | Fit |
|---|---|---|---|---|---|
| **Design / product engineer** | Owns the part: function, sizing, material, interfaces, tolerances | Requirements, interfaces → model, drawing, release request | Meets requirements, margins, fits; released | A feature that is sound and whose intent is stated; release is the big one | ● D1/D7, ◐ D2 |
| **CAD designer / drafter** | Builds the model and the drawing to the engineer's direction | Sketches, markups, specs → model, sheets | Model matches direction; sheet passes the checker | "Ready for check" | ● (the drawing path) |
| **Industrial designer** | Form, ergonomics, appearance surfaces (class-A) | Brief, sketches → surface model to engineering | Look, continuity (G2/G3), brand | A form freeze | ○ |
| **Systems engineer** | Requirements, flow-down, interfaces, budgets (mass, power, envelope) | Customer/programme needs → requirements per part | Every requirement traced to evidence | A requirement verified | ○ (see Omnigraph "hardware program") |
| **CAE / analysis engineer** | Meshing, FEA, CFD, thermal, modal; idealises geometry first (defeature, mid-surface) | Model, loads, BCs → margins, report, design advice | Mesh converged, margins ≥ allowable | An analysis run tied to a model state | ○ (website: "building") |
| **Tolerance / GD&T specialist** | Datum schemes, stack-ups (worst case, RSS), FCFs | Assembly, function → tolerance scheme | Stack-up closes; datums functional | Scheme agreed | ◐ GD&T verdicts, no stack-up |
| **Drawing checker** | Independent check of every sheet: dims, tolerances, standards, title block | Sheet → redlines | Every redline closed | "Checked" stamp | ◐ sheet certificate does the mechanical part; no redline loop |
| **Design lead / chief engineer** | Runs design reviews (concept, PDR, CDR), approves | Review packs → decisions | Risks closed, approved | A review gate passed, with a signature | ○ |
| **Configuration / PLM manager** | Part numbers, revisions, BOM, change orders, release states | Change requests → released data | Right revision, right BOM, traceable | A released revision (frozen) | ○ |
| **Standards / library engineer** | Standard parts, templates, house rules, design guides | Standards → libraries | Library correct and current | A library version | ◐ kb / playbooks are this |

## B. Manufacturing and quality

| Role | Does | Receives → hands on | Checks / "done" means | A checkpoint is | Fit |
|---|---|---|---|---|---|
| **Estimator** | Quotes: material, route, cycle time, price | RFQ pack (STEP + drawing) → quote | Data consistent, makeable, cost drivers known | Quote issued against a revision | ◐ feature/tolerance extraction |
| **Buyer / procurement** | Sources parts, sends RFQ packs, manages suppliers | Released data → POs | Supplier has the right revision | Revision sent | ○ |
| **Manufacturing engineer** | Process route, DFM/DFA feedback, line and cell design | Released design → routing, feedback | Makeable at rate and cost | Process frozen | ◐ DFM check |
| **CAM programmer / process planner** | Setups, fixtures, operations, toolpaths | Model + drawing → NC programs | Every feature reachable, simulated clean | Program proven on the machine | ○ (needs datums/features: exist) |
| **Machinist / operator** | Runs the job | Traveller, drawing, program → parts | Parts in tolerance | First-off approved | ○ (reads the drawing) |
| **Tool, die and mould designer** | Core/cavity, dies, electrodes from the part | Part + process → tooling model | Tool makes the part (draft, shrink, parting) | Tool design freeze, T1 samples | ◐ mould verb |
| **Fixture / jig designer** | Holds the part for machining, welding, inspection | Part + datums → fixture | Locates on the drawing's datums | Fixture approved | ◐ datums |
| **Quality engineer** | Control plan, PFMEA, SPC, capability (Cpk) | Drawing, process → plans | Critical characteristics controlled | Plan approved | ○ |
| **Inspector / CMM programmer** | Balloons the drawing, programs the CMM, measures, writes the FAI report (AS9102) | Drawing + parts → inspection report | Every characteristic measured in limits | First article accepted | ◐ the sheet knows every characteristic |
| **Supplier quality engineer** | Approves suppliers' parts (PPAP in automotive) | Supplier's submission → approval | All elements present and in spec | PPAP approved | ○ |
| **MRB / non-conformance owner** | Decides on out-of-spec parts: use-as-is, rework, scrap | NCR → disposition; recurring → change request | Decision recorded and justified | A signed disposition | ○ |
| **Assembly technician** | Builds the assembly | Assembly drawing, BOM, work instructions → product | It goes together in the stated order | Build complete | ◐ interference, no work instructions |

## C. After the product ships

| Role | Does | Fit |
|---|---|---|
| **Service / field engineer** | Diagnoses field failures, spares, repair procedures; raises change requests | ○ |
| **Technical writer / illustrator** | Manuals, exploded views, parts catalogues from the model | ◐ renders, no exploded views |
| **Reliability engineer** | Failure data, FMEA updates, life testing | ○ |
| **Sustainment / obsolescence** | Keeps old revisions buildable for decades (aerospace, defence) | ◐ immutable history replays any revision |

## D. Exploring and learning

| Role | Does | Wants | Fit |
|---|---|---|---|
| **R&D / advanced engineer** | Concepts, feasibility, new principles | Cheap branches, compare, sweeps | ◐ branches, no compare/sweep |
| **Startup hardware generalist** | All of the above, alone | One tool that holds the whole chain | ◐ |
| **Consultant / design services** | Designs to a client's standards, hands over data | Client templates, clean handover | ◐ |
| **Student / new engineer** | Learns why a design is the way it is | The story: intent, history, standards | ● chapters, lens, lineage |
| **Researcher / AI lab / agent operator** | Runs agents, evals, RL episodes | Every attempt recorded and scored | ◐ eval + runner |

## E. Management

| Role | Wants |
|---|---|
| **Programme / project manager** | Which parts are at which stage; what blocks release |
| **Engineering manager** | Load per engineer, review backlog, quality escapes |

---

## How the role changes by industry

The same role works differently under each industry's rules. These overlays
decide which gates are mandatory and who signs.

| Industry | Governing practice | What it adds |
|---|---|---|
| **Automotive** | APQP (5 phases), PPAP (18 elements), PFMEA, control plans, IATF 16949 | OEM–Tier 1–Tier 2 chain: the supplier designs or makes to the OEM's data and proves it with a PPAP; drawings carry special-characteristic symbols |
| **Aerospace** | AS9100, AS9102 FAI, configuration control, design-organisation approval, certification by authority | Traceability is law: every revision reproducible for decades; every characteristic ballooned and measured; changes classed major/minor |
| **Medical devices** | Design controls (21 CFR 820 / ISO 13485), risk management ISO 14971 | Design history file; verification ("built right") vs validation ("right thing"); every design input traced to an output |
| **Process / oil & gas / pressure** | ASME, EN codes (EN 1092-1, ASME B16.5, B31.3), PED | Parts are specified by standard, not designed: the flange case; pressure ratings, material certs |
| **Consumer electronics** | EVT / DVT / PVT builds | Speed; tooling-driven (moulds); many fast iterations |
| **Industrial machinery** | Configure-to-order, variants, ISO 12100 safety | Families and variants (D3) dominate; BOM per configuration |
| **Defence** | MIL standards, technical data packages | Long life, sustainment, strict data packages |
| **Job shop / contract manufacturer** | Customer's drawing is the contract | Quote → make → inspect; they never own the design |

## How it changes by company type

- **OEM** — owns the design and the requirements; runs the gates.
- **Tier-1/2 supplier** — receives data (often STEP + drawing: the bracket
  take), designs to it or makes to it, proves conformance back.
- **Job shop** — receives drawings, quotes, makes, inspects.
- **Design consultancy** — designs to the client's standards, hands over.
- **Startup** — one person does every role; gates are informal until a
  customer or regulator imposes them.

---

## What this says for Roshera

1. A checkpoint is "a named state + evidence + who may pass it". The roles
   differ in the evidence they need and in whether a kernel or a person
   signs. Kernel-signable: sound, values match a standard, sheet complete,
   GD&T in spec, interference-free, DFM pass. Person-signed: spec agreed,
   review passed, checked, released, change approved, disposition.
2. Industry overlays decide which person-gates are mandatory (aerospace:
   FAI and configuration control; automotive: PPAP; medical: design
   controls). A gate set should be a configurable template, not hard-coded.
3. The Blackboard is read by different roles: an engineer wants the
   feature cards and gates; a checker wants the sheet and redlines; an
   inspector wants the characteristic list; a manager wants the stage strip.
4. Roshera is strongest for: design engineer (standard and imported parts),
   drafter, standards/library, student, agent operator. The nearest big
   wins: drawing checker (redline loop on top of the sheet certificate),
   inspector (ballooned characteristic list / FAI from the sheet), and
   configuration (revision + release + change approval).
