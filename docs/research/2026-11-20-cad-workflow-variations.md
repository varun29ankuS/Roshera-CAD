# CAD workflow variations — designing, manufacturing, exploring

Research note, 2026-11-20. Purpose: recognise every variation of how people
work with a part *before* deciding what the Blackboard, its cards and its
checkpoints must do. Each variation lists: the trigger, the steps, what the
person needs at each gate, and what "a checkpoint" means to them.

The three points of view want different things from the same history:

| POV | Core question | Speed vs rigour | What a checkpoint is to them |
|---|---|---|---|
| Designing | "Is this the right part, and is it right?" | Rigour, with fast iteration inside a feature | A save point that passed its checks; release = a frozen, signed revision |
| Manufacturing | "Can I make exactly this, and prove I did?" | Rigour, zero ambiguity | A *locked revision* — which one am I making — plus evidence of conformance |
| Exploring | "What could this be? What happens if…?" | Speed; rigour only at the end | A *branch point* to compare from; one gate only: promoting an option to the real design |

---

## 1. Designing

### D1 — Part from a standard
*Trigger:* "DN50 PN16 weld-neck flange to EN 1092-1."
*Steps:* identify standard/type/size/class → look up every governed value →
ask for what the standard does not give (hub, finish) → build → verify →
datums from function → drawing.
*Gates:* spec complete (no guessed value) · sound · values match the table ·
sheet sound + complete.
*Wants:* never a remembered number; departures from the standard named.
*Roshera today:* strong — playbooks, kb_lookup, refusal when the table is
silent (the flange take).

### D2 — Part from function / requirements
*Trigger:* "a bracket that holds a 12 kg motor to this rail, 3 g shock."
*Steps:* requirements (loads, envelope, interfaces, environment, process) →
concept sketch → size the critical sections by hand calc → model → analysis
(FEA) → iterate geometry against margin → DFM → drawing.
*Gates:* requirements agreed · concept chosen · margins met (stress,
deflection, frequency) · DFM clean · drawing.
*Wants:* the loop from analysis back to geometry to be short; the reason for
each dimension recorded (it came from a margin, not taste).
*Roshera today:* weak — no simulation loop yet (website: "building").

### D3 — Variant of a family
*Trigger:* "same flange, DN80" / "bracket, 20 % longer".
*Steps:* pick the family member → change the driving values → everything
downstream regenerates → re-verify → re-issue drawing.
*Gates:* every downstream feature still sound · drawing re-certified ·
new part number vs new revision decided.
*Wants:* one edit, everything follows; a diff of what changed.
*Roshera today:* partial — timeline replay/resketch, parameters endpoint;
no family table.

### D4 — Engineering change to a released part (ECR → ECO)
*Trigger:* field failure, cost-down, supplier request.
*Steps:* change request (why) → impact: where-used, tooling, stock,
inspection → decide (approve/reject) → change on a branch → verify →
re-review → revision B released → notify manufacturing.
*Gates:* impact assessed · approved by a person · re-verified · released.
*Wants:* what exactly changed (geometry diff, drawing diff), who approved,
and that revision A is still reproducible.
*Roshera today:* branches + immutable history exist; no revision/release
state, no approval record, no diff view.

### D5 — Top-down assembly
*Trigger:* new product: envelope and interfaces first.
*Steps:* skeleton/layout (planes, axes, envelopes) → parts derived from the
skeleton → change the layout, parts follow.
*Gates:* interfaces frozen per partner part · no interference.
*Wants:* the layout to drive the parts; who owns which interface.
*Roshera today:* not modelled (assembly is bottom-up mates).

### D6 — Bottom-up assembly
*Trigger:* parts and bought-in components exist; put them together.
*Steps:* instance → mate against datums/features → interference → stack-up
→ assembly drawing + BOM.
*Gates:* fully constrained · zero interference at a stated epsilon · stack-up
clears · BOM complete.
*Roshera today:* instances, mates, SE(3) solve, interference — yes; BOM and
assembly drawing thin.

### D7 — Imported / supplier part (STEP in)
*Trigger:* "here is the supplier's STEP; we need our drawing" (the bracket
take).
*Steps:* import → is it sound? → what is it (feature recognition: bores,
holes, patterns) → ask what must not be guessed (which bores are fits) →
datums → GD&T → drawing.
*Gates:* imported sound · features recognised · human-confirmed critical
features · sheet sound + complete.
*Wants:* no remodelling; the drawing to be right about a model they did not
build.
*Roshera today:* strong path, importer at 46/81 on CADGenBench.

### D8 — Process-specific design
Sheet metal (bends, flat pattern, K-factor), castings (draft, parting line,
machining allowance), mouldings (draft, wall, ribs, gates), weldments (cut
list, weld symbols), machined from billet (stock size, setups).
*Gates:* the process's rules pass (DFM pack).
*Roshera today:* DFM packs for FDM/SLA/SLS/injection/casting/sheet metal as
guidance; no unfold, no draft analysis certificate.

---

## 2. Manufacturing

### M1 — Quote (RFQ)
*Trigger:* a STEP + drawing arrives from a customer.
*Steps:* what is it → material and stock size → process route → which
tolerances are tight (cost drivers) → cycle time → price and lead time.
*Gates:* data complete and consistent (model agrees with drawing) · no
unmakeable feature.
*Wants:* fast, trustworthy feature and tolerance extraction; the tight ones
flagged.
*Roshera today:* the readable layer + hole/feature detection could serve
this; no quoting view.

### M2 — DFM feedback to the designer
*Trigger:* "we can't hold that" / "this needs a 5-axis setup".
*Steps:* review → redline → designer changes → re-check.
*Gates:* every redline answered (accepted or rejected with reason).
*Wants:* comments pinned to faces/dimensions, a loop that closes.
*Roshera today:* DFM check exists; no redline/comment-on-geometry loop.

### M3 — Process planning and CAM
*Trigger:* released revision.
*Steps:* setups and fixturing datums → operation sequence → tools →
toolpaths → simulate → post to machine.
*Gates:* the datums the machinist fixtures on match the drawing's datums ·
every feature reachable.
*Wants:* datums from function (the policy already says so), stock model.
*Roshera today:* out of scope; the data it needs (datums, features) exists.

### M4 — Inspection planning and first-article inspection
*Trigger:* first parts off the machine.
*Steps:* balloon every characteristic on the drawing → inspection plan →
CMM program from GD&T → measure → report (AS9102 FAI in aerospace) →
accept / reject.
*Gates:* every characteristic ballooned and measured · each within limits.
*Wants:* a characteristic list straight from the drawing; measured values
compared to the certified nominal.
*Roshera today:* the sheet already knows every dimension and FCF
(dimension_edit keys) — a ballooned characteristic list is close.

### M5 — Tooling and fixture design
Mould, die, fixture derived from the part (core/cavity, parting line).
*Roshera today:* a mould verb exists on the timeline (`/api/timeline/mould`).

### M6 — Non-conformance and concession
*Trigger:* a part measured out of spec.
*Steps:* NCR → material review board → use-as-is / rework / scrap → if
recurring, an ECR back to design (D4).
*Gates:* a person decides, recorded.
*Wants:* the measured part against the nominal; the decision traceable.

### M7 — Model-based definition
The 3D model with PMI replaces the drawing as the contract.
*Wants:* the same certainty the sheet certificate gives, on the model.
*Roshera today:* GD&T lives on the model already; PMI export not built.

---

## 3. Exploring

### E1 — Concept exploration
*Trigger:* "show me three ways to do this bracket".
*Steps:* several options quickly, rough → compare (mass, size, cost, part
count) → pick → promote to a real design (D2).
*Gates:* none while exploring; ONE at promotion (the chosen option gets
the full design gates).
*Wants:* branches that cost nothing, side-by-side compare, no ceremony.
*Roshera today:* branches exist; no compare view; gates (checkpoint before
mutation, verify before next checkpoint) apply everywhere — friction here.

### E2 — What-if / trade study
*Trigger:* "what does 2 mm thinner do to mass and stress?"
*Steps:* vary one or more parameters → evaluate each → table/plot →
decide.
*Gates:* each point evaluated by the same measure.
*Wants:* a sweep, not twenty manual edits; results as a table/plot.
*Roshera today:* parameters + replay could drive it; no sweep.

### E3 — Understanding an existing design
*Trigger:* a new engineer, a reviewer, an imported or inherited model:
"what is this and why is it like this?"
*Steps:* walk the history → read intents → inspect features → ask.
*Wants:* the story: each feature's intent, author, what it depended on.
*Roshera today:* strong ingredients — chapters (intent, author, sound),
lens, lineage, feature graph, scrub pictures; the Timeline Map is the plan.

### E4 — Feasibility check
*Trigger:* "will a 40 mm bearing fit in this housing?"
*Steps:* quick envelope / interference / clearance measurement → yes/no.
*Wants:* an answer in seconds, with the measurement shown.

### E5 — Agent / AI-lab exploration
*Trigger:* an eval run, an RL episode, an agent trying approaches.
*Steps:* many attempts on branches → each scored by certificates → the
best kept, all recorded.
*Wants:* every attempt recorded and scored; nothing a human has to
babysit.
*Roshera today:* AGENT-EVAL, the roshera-rl runner (mock-tested), branches.

---

## Other points of view present in the same history

- **Checker / reviewer** — redlines a drawing or model; needs to mark up and
  see the response (D4, M2, drawing check).
- **Quality** — inspection, NCRs, audits (M4, M6).
- **Supplier / purchaser** — receives the data pack: model, drawing,
  revision, material certs.
- **Programme / project lead** — which parts are at which stage.
- **Agent operator** — what each agent did, on which branch, scored how.

---

## What this says about checkpoints and the Blackboard

1. One history, different lenses. A designer reads features and gates; a
   machinist reads the released revision and its characteristics; an
   explorer reads branches side by side.
2. Gates are POV-dependent. Exploring wants none until promotion; designing
   wants automatic kernel gates per feature and human gates at release;
   manufacturing wants the revision locked and conformance evidence.
3. A checkpoint is always "a named moment + the evidence at that moment".
   What differs is which evidence matters and who may pass it:
   kernel-only (sound, sheet complete) vs person-signed (spec agreed,
   change approved, release, use-as-is).
4. The variations Roshera already serves best are D1, D7 and E3; the
   biggest gaps for real users are D4 (revision/approval/diff), M4
   (characteristic list / FAI) and E1/E2 (compare and sweep).
