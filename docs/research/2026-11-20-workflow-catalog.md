# Workflow catalog

2026-11-20. The structured list behind workflow suggestion: each workflow
has an id, how a request sounds when it wants it, its stages, and each
stage's gate with WHO signs it (K = the kernel certifies it; P = a person
signs it; K→P = the kernel produces the evidence, a person signs).
Background: `2026-11-20-cad-workflow-variations.md` (the workflows in
prose) and `2026-11-20-cad-roles-in-industry.md` (the people).

First identity: **aerospace supplier engineer** (marked ★ where the
workflow is theirs).

---

## Design

| Id | Workflow | Sounds like | Stages → gate (signer) |
|---|---|---|---|
| D1 | Part from a standard | "DN50 PN16 weld-neck flange to EN 1092-1" | Spec: standard, type, size, class (P) → Values from the table, none guessed (K) → Build (K sound) → Datums (P) → Drawing (K sound + complete) |
| D2 | Part from requirements | "a bracket holding 12 kg at 3 g" | Requirements agreed (P) → Concept chosen (P) → Hand sizing (K→P) → Build (K) → Analysis margins (K→P) → DFM (K) → Drawing (K) |
| D3 | Variant of a family | "same flange, DN80" / "20 % longer" | Member chosen (P) → Regenerate (K every feature sound) → Diff shown (K) → New number or new revision (P) → Drawing re-certified (K) |
| D4 ★ | Engineering change | "change the bore to Ø42", "rev B" | Change request with reason (P) → Impact: where-used, drawings, inspection (K) → Approved (P) → Change on a branch (K) → Diff (K) → Re-check (P) → Release rev B (P) → Delta first article (see M4) |
| D5 | Top-down assembly | "lay out the gearbox first" | Layout (P) → Interfaces frozen (P) → Parts derived (K) |
| D6 | Bottom-up assembly | "put these together" | Instances + mates (K constrained) → Interference at ε (K) → Stack-up (K→P) → BOM (K) → Assembly drawing (K) |
| D7 ★ | Supplier / imported part to drawing | "here is the customer's STEP, we need the drawing" | Import (K sound) → Recognise features (K) → Confirm what must not be guessed: fits, key characteristics (P) → Datums (K→P) → GD&T (K verdicts) → Sheet (K sound + complete) → Check (P) → Release (P) |
| D8 | Process-specific design | "sheet-metal bracket", "cast housing" | Process chosen (P) → Process rules (K / guidance) → as D1/D2 |

## Manufacturing and quality

| Id | Workflow | Sounds like | Stages → gate (signer) |
|---|---|---|---|
| M0 ★ | Data-pack review (contract review) | "we got this package from the OEM, can we take it?" | Revision identified (K→P) → Model agrees with drawing (K) → Key characteristics listed (K→P) → Gaps and questions listed (K→P) → Accept / query the OEM (P) |
| M1 | Quote | "how much to make 50 of these?" | Data consistent (K) → Stock + route (K→P) → Tight tolerances flagged (K) → Quote (P) |
| M2 | DFM feedback | "can we make this?" | DFM (K) → Redlines (P) → Each answered (P) |
| M3 | Process planning / CAM | "set it up for machining" | Setups on the drawing's datums (K→P) → Reach (K) → out of scope today |
| M4 ★ | First article inspection (AS9102) | "balloon it", "FAI", "inspection plan" | Characteristic list from the sheet, every one numbered (K) → Method per characteristic (P) → Measured values entered (P) → Each against its limits (K) → Forms 1/2/3 (K→P) → Accepted (P) |
| M5 | Tooling / fixture | "mould for this", "fixture on datums A/B" | Derived from the part (K) → Design freeze (P) |
| M6 ★ | Non-conformance | "part 7 measured Ø40.03" | Measured vs nominal and limits (K) → Disposition: use-as-is / rework / scrap (P) → If recurring, raise D4 (P) |
| M7 | Model-based definition | "no drawing, PMI only" | GD&T on the model (K) → PMI export (K) |

## Exploring and understanding

| Id | Workflow | Sounds like | Stages → gate (signer) |
|---|---|---|---|
| E1 | Concept options | "three ways to do this bracket" | Options on branches (no gates) → Compare (K) → Promote one (P) — then its design gates |
| E2 | What-if / sweep | "what does 2 mm thinner do?" | Parameter range (P) → Each point evaluated by the same measure (K) → Table (K) → Decide (P) |
| E3 ★ | Understand a design | "what is this part?", "why is this bore here?" | Read only: history, intents, features, measurements (K) — no gates |
| E4 | Feasibility | "will a 6204 bearing fit here?" | Measure (K) → Yes / no with the measurement |
| E5 | Agent episodes | eval, RL | Attempts on branches (K scored) → Keep best (K) |

---

## How a request is matched to a workflow

1. Read the request for: what arrived (a STEP, a drawing, a standard
   name, a measurement), what is wanted (a drawing, a part, an answer, a
   report), and the verb (make, change, check, explain, compare).
2. Propose the best match and one alternative, with the stages it will run,
   on a plan card — the person confirms or picks.
3. Unclear request: ask the one question that separates the candidates
   (for example: "did a drawing come with the STEP?" separates D7 from M0).
4. The identity filters and orders the suggestions (an aerospace supplier
   engineer sees D7, M0, M4, D4, M6 first; their gate template adds AS9102
   and configuration-control gates).
