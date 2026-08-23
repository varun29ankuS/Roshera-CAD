# Agent surface — designed, measured, not yet built

Written 2026-08-22, at the end of a long session on the Blackboard, the
lineage view and the kernel. Everything **implemented** that day is in the
commit messages, which are the record; this file holds only the work that was
designed and verified but not written, so it survives a restart.

Specifications here came from ox-alpha against real measurements. Where a
claim was checked against the live system, the number is quoted. Where it was
not, it says so.

---

## 1. Tool calls in the Blackboard — run-folding and turn sections

**The defect.** One turn renders seven near-identical rows:

```
Solid · … · valve_pocket_cutter_R2   — 8 steps
Solid · … · valve_pocket_cutter_R2   — 24 steps
Solid · … · valve_pocket_cutter_R2   — 16 steps
```

The titles collide, the only distinguishing number is a step count, and
nothing groups them into the one thing the agent was doing.

**Turn sections.** The session store already holds `turns`. A hairline and an
11px caps micro-label (`TURN 4`) opens each group. Free structure, honestly
held — no new data.

**Run-folding.** Consecutive calls whose titles match byte-for-byte fold into
one row: the worst status in the run as the verdict glyph, the title once
(middle-truncated, verbatim), `×6`, and a tally when mixed (`5 ok · 1 failed`).
The row is a `<button>` that expands to the originals, each still individually
expandable into the existing payload pipeline.

**Scope the claim carefully.** The folded row asserts *six calls, same title* —
never "same operation". Arguments are not on the wire, so identical titles may
hide different parameters.

**The live call never folds.** It stays its own row with the working glyph, so
the tail of the stream always shows exactly one moving thing. When it
completes it joins the run above. That fold is the animation; no spinners
beyond the honest glyph.

Demote the step counts out of the collapsed row into the expanded payload
header — on the row they compete with the only number that matters.

If goose ever emits `parentToolCallId` (it exists in ACP; absent from our
capture), nest semantically instead of folding lexically.

---

## 2. ChoiceCard — specified, and correctly parked

**It cannot be populated today, and that is the finding.** `ask_choice`'s
question and options travel in the tool's *arguments*, and `tool_call` carries
neither arguments nor structured content. There is literally nothing on the
wire to render but a title. Interim honest behaviour is what already happens:
the human answers in prose.

When the wire cooperates: question verbatim at the top; options as stacked
full-width `<button>`s with labels verbatim and number-key hints; **no
preselected row**, because a default biases a judge; selection dispatches on
the channel the question arrived on, then the card collapses to an audit line
(`You chose 2 mm · 14:33:10`). It shares the sticky strip and the wait clock
with the permission card, both of which now exist.

---

## 3. What the agent must send for any of this to be honest

Named exactly. Nothing else is wanted — explicitly **not** raw argument dumps,
**not** server clocks (the permission wait clock is ours), **not** heartbeats.

1. `tool_call.startedAt` / `endedAt` — unlocks honest durations. Until then no
   timers on calls, which is why none render.
2. `tool_call.detail` — one agent-curated human string saying what *this*
   invocation was. Unlocks telling eight identical calls apart.
3. `tool_call.parentToolCallId` — semantic nesting over lexical folding.
4. On `session/request_permission`: `toolCallId`, `title`, `description`, and
   `options[].name`. Today only `{optionId, kind}` arrives, which is why the
   permission card renders kinds as verbs and says so when the agent sent no
   description.
5. Allowed-value enumeration on config options — until then the model chip is
   a facts viewer, never a fake dropdown.
6. Structured choice delivery for `ask_choice` — either `tool_call_update`
   content carrying `{question, options[], multiSelect}`, or a dedicated
   `session/request_choice` mirroring `request_permission` (preferred, for
   symmetry).
7. Optional: `tool_call_update.progress {current, total, unit}`. Only then may
   any bar render. Absent, none — which is already the rule.

---

## 4. Lineage lanes — the remaining tier

Shipped: lanes, termini, eras, the census, focus-a-row. Not shipped:

- **Hover a tick → seek the viewport to that op's state.** One new kernel call
  (replay-to-sequence), nothing invented in the data. It is the difference
  between reading that the part evolved and watching it.
- **Cluster collapse for dense lanes.** 62 of 105 lanes carry a single tick.
  They are feeders (a cutter born, consumed, gone). Drawing them as short
  stubs at their merge glyph would compress the picture further. Not urgent —
  the current ranking already puts them below the spine.

---

## 5. Measured facts worth not re-deriving

These cost real time to establish. Each is quoted from the live system.

- **Solid ids are not identity.** Of 34 ops carrying both inputs and outputs,
  **zero** preserved an id. Chain on output→input references.
- **Solid ids are recycled.** `solid:0` produced 4×, `solid:2` 5× on one
  document, because a workspace reset restarts numbering. The key is
  (id, producing op).
- **The lineage graph is not a forest.** `solid:1` has **8** consumers.
- **44 inputs** reference a producer outside the served window; **3 ids** are
  re-produced while a previous instance is still live.
- **The Blackboard's KaTeX path works.** remark-math + rehype-katex are wired
  and the stylesheet loads; **zero** KaTeX nodes had ever rendered, because
  `$math$` was documented in one tool description out of 109 and goose's ACP
  prose — the dominant path — never reads it. The description now teaches it;
  goose's own prose is still untouched.
- **`cad-panel-header`'s `@apply px-3` beats a `px-2` utility.** The override
  reads as applied and measures 12px. Copy the type out rather than fight it.
- **`getComputedStyle().marginTop` resolves `auto` to pixels.** An `mt-auto`
  footer reports `"285px"`, never `"auto"` — a keyword check silently never
  fires.

---

## 6. Open, needing a human

- **Rebuild `api-server`.** The MCP reports *"compiled but not in kernel:
  part_rename"* — the running binary predates the registry entry. The tool
  works (verified live: a boolean result named `bushing sleeve`); only the
  drift warning is stale.
- **Rotate the OpenRouter key.** It was passed in plaintext during the session
  and drove every ox call.
- **`ROSHERA_DEV_INSECURE=1`** is set on the running server.
- **The piston's residual sliver.** The antipodal bridge is fixed (wings 4→0,
  aspect 456→172) but `max_normal_deviation_deg` is still 83.6° and the
  minimum angle got *worse*. A second sliver sat under the first.
  `tests/cross_bore_mesh_wings.rs` is the pinned RED and carries the numbers.
