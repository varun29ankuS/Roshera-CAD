# Conversation UI design: the Blackboard (2026-11-18)

This design follows from `CONVERSATION_UI_AUDIT.md`. It keeps the founder's rules: the
viewport is the work (a transparent, closable overlay with no blur); panel headers hug;
text never drops below 11 px; numbers live in the status bar; cards, not paragraphs; tick or
cross, not prose; and an engineer-first look (a SolidWorks/Fusion feature-tree register,
not a chat app).

The organising idea is that **every agent turn ends in the kernel's answer**. The reader
needs four things at a glance: what was asked, what was built, whether the kernel certified
it, and what to look at if it did not.

## 1. Message grouping: the turn

A **turn** is the run of non-user lines between two user lines. Only `user` is a reliable
boundary, because system lines round-trip as `agent`. The turn's work is the run. It is not
labelled as "the answer to prompt N": queued asks sit adjacent, and their work lands after
the last of them.

The turn has three layers:

```
 ┌ you ───────────────────────────────────────────────┐
 │ Add four M6 bores on the flange PCD   [Flange·face 56 ×]│   ← ask + attached selection chips
 └────────────────────────────────────────────────────┘
 ◇ Agent  Planned: 4× bore Ø6.6 on PCD Ø80, then verify     ← intent (agent prose, first line)
   ⚙ bore ×4 · Difference ×4                      ▸ steps   ← build strip (collapsed)
   … reply prose (capped, "Show full message") …
 ─ Kernel evidence  ✓ sound  ⊘ DFM inconclusive   ▸ ─────   ← TURN EVIDENCE STRIP (verdict)
```

The **turn evidence strip** closes every turn that carries evidence or changed geometry. It
holds one mark per verdict-carrying card, in order. Each mark is a glyph plus a word, never
a count:

| Card | pass | fail | can't-say |
|---|---|---|---|
| soundness | `✓ sound` | `✗ unsound` | `⊘ soundness not run` |
| dfm | `✓ DFM pass` | `✗ DFM violations` (report) / `✗ DFM violation` (one rule) | `⊘ DFM inconclusive` |
| fcf | `✓ GD&T in spec` | `✗ GD&T out of spec` | `⊘ GD&T not evaluable` |
| merge | `✓ merged` | `✗ merge failed` | `⊘ merge needs a decision` |
| refusal (typed card or `Server refused:`) | — | — | `⊘ refused` |

The states are:

```
 ─ Kernel evidence  ✓ sound  ✓ DFM pass ──────────────────    all certified (emerald marks)
 ─ Kernel evidence  ✓ sound  ✗ GD&T out of spec ──────────    a failure: red mark; strip rule red
 ─ Kernel evidence  ⊘ refused ────────────────────────────    can't-say: amber/sky
 ─ Kernel evidence  ⊘ no certificate shown ───────────────    geometry was built ("Created …")
                                                              but no certificate reached the board
 (no strip)                                                   a prose-only turn: nothing to certify
```

- Clicking a mark scrolls to the card's own line, so the evidence opens where it was written.
- The strip's rule takes the **worst** verdict: fail beats can't-say beats pass. A turn can
  never read clean because one card passed while a later one failed.
- "No certificate shown" is the honesty case. The kernel did work and the agent did not
  forward its certificate. The strip must not read as certified.
- While the turn is still streaming, "no certificate shown" is held back. The kernel echoes
  `Created …` at the first bore and the agent posts `verify_part` at the end. An amber mark
  that flashed on every build would teach the engineer to ignore amber. Real card marks show
  live.
- The strip is derived from line text, not from the unpersisted `turnStatus`, so it survives
  a reload.
- Known limitation: a toolbar create (`Created cube (…)`) is a non-user line, so it joins the
  preceding agent run and can add "no certificate shown" to a turn the agent did not do.
  **Backend ask:** a `System` author on the blackboard wire enum (it would also fix audit
  §2.4), so that human-initiated bookkeeping can become its own boundary.
- Next: a collapsed checkpoint section shows its turns' worst verdict glyph in its header.
  Today, collapsing a section hides its strips.
- **Turn ended ≠ certified.** The reply's end-of-turn glyph becomes neutral (muted tick for
  completed). Emerald is reserved for certificates. A cancelled turn stays amber and a failed
  turn stays red.

Checkpoints: the strip is computed across the **whole** notebook, not per section, and is
drawn after the turn's last line, wherever that line sits. A mid-turn checkpoint can then
split the prose, but the verdict always lands at the end of the work it covers.

## 2. Compact tool-call rendering (needs the backend)

Target, for a provider that emits `tool_call` frames, or once activity carries params:

```
   ⚙ create_cylinder  r 3.3 mm · h 12 mm · at (40, 0, 0) mm        ✓ done   ▸ result
   ⚙ boolean          difference · Flange − bore 1/4               ✓ done   ▸ result
   ⚙ verify_part      Flange                                       ✗ unsound ▸ certificate
```

- Parameters in engineering units (mm, °, Ø), with the units store choosing mm or in.
- The status chip is the tool's verdict when its result is a card (soundness → sound or
  unsound), and the transport status otherwise.
- **Backend asks (not frontend):** (a) forward `rawInput` on ACP `tool_call` frames on the
  claude-code bridge; or (b) add `params` (and `result_kind`) to `GET /api/acp/activity`'s
  `recent_operations`; and (c) persist the per-turn operation ledger as a board line at turn
  end. The current `ToolCallRow` is dead on the live provider (audit §0).

## 3. Refusals: a typed reason and the fix

```
 ┌ ⊘ Refused — fillet r 4 mm on edge 112      [BLEND_ENVELOPE] [not retryable] ┐
 │ Reason   radius exceeds the adjacent face's minimum curvature radius 3.2 mm │
 │ Fix      ▸ reduce radius to ≤ 3.2 mm    ▸ fillet the face pair instead      │  ← option buttons
 └─────────────────────────────────────────────────────────────────────────────┘
```

- This already exists for fenced refusals (`RefusalCard`). Missing: "next" options as
  **buttons** that send the option as the next turn (reuse `ChoiceButtons`).
- `Server refused: …` (WS `Error`) becomes a refusal card when the frame carries the error
  catalog fields. **Backend ask:** send `error_code`/`hint`/`retryable` on the WS `Error`
  frame, as the REST envelope already does.
- A refusal turn on the ACP error path must render sky, not red (it is a result). This is
  already true when it is fenced. The prose branch should name the stable code when one exists.

## 4. The user's selection attached to the ask

```
 Composer:
 ┌──────────────────────────────────────────────────────────────┐
 │ [◉ Flange · face 56 ×] [◉ Flange · edge 112 ×]   Agent sees ◉ │   ← chips from user-context store
 │ fillet this edge 2 mm                                         │
 │ Enter sends · Shift+Enter new line                     [Send] │
 └──────────────────────────────────────────────────────────────┘
 Record:
                    ┌ fillet this edge 2 mm  [Flange · edge 112] ┐   ← chips frozen on the line
```

- Chips come from `useUserContextStore().acknowledged.selection`, which is what the agent
  would actually be told, named through `agentSeesLabel`'s `describe`. Removing a chip drops
  it from this ask only. It never changes the viewport selection.
- On send, the kept chips are frozen into the line as a `roshera:context` fence (kernel ids:
  `object_id`, kind, id). A new card kind renders it as chips. The agent's prompt carries the
  same fence, so "this edge" is resolvable after the live context has moved on.
- **Decision needed (Varun):** this changes the text the agent receives. It is frontend-only,
  but it is a policy question (`roshera-agent-policy.md` would describe the fence).

## 5. "Agent sees" state

- It stays in the status bar (`AgentSeesChip`). That is the right home under the numbers rule.
- In the panel header: one eye glyph that lights while the agent reads context
  (`isLooking`), with no text. The composer's chip row ends with `Agent sees ◉`, which
  toggles sharing (the same store).

## 6. Plan accept/reject

ACP `plan` frames are dropped today. Target:

```
 ┌ Plan — 4 steps                                      [Accept all] [Reject] ┐
 │ ☐ 1 sketch PCD Ø80 on top face                       [✓] [✗]             │
 │ ☐ 2 bore Ø6.6 ×4                                     [✓] [✗]             │
 │ ☐ 3 verify_part                                                          │
 └──────────────────────────────────────────────────────────────────────────┘
```

Entries come from `plan.entries[].content/status/priority`. Accepting or rejecting sends the
decision as the next turn (like choices) and records it in the line. The "Plan — 4 steps"
count is an identifier of the plan's length, not a measurement. **Backend ask:** confirm the
claude-code bridge emits `plan`. If it does not, the agent posts a `roshera:plan` fence (an
MCP tool like `ask_choice`).

## 7. Streaming and progress for long operations

- Keep `TurnStatusLine` (verb, activity, Stop). Move elapsed, tokens and op count to the
  status bar (the numbers rule). In the panel: `✻ Machining… ⎿ boolean difference`.
- Each "Created …" echo that arrives mid-turn joins the live build strip, which grows in
  place instead of appending lines.

## 8. Keyboard and scroll

- **Follow the tail only when the reader is at the tail** (within 48 px). Scrolling up
  pins the view. A `↓ Jump to latest` pill appears when new lines arrive while pinned.
  Sending a prompt or clicking the pill resumes following.
- Enter sends, Shift+Enter makes a new line, ArrowUp/Down recall history (exists). Add: Esc in
  the composer = Stop when a turn runs. Ctrl/Cmd+↑/↓ = previous or next turn evidence strip.

## 9. Density

- Rows at 11 px minimum (fixing the 10.5 px composer hint). Body prose stays 13 px. Evidence
  cards are 11–12 px.
- One turn = ask box + prose + one strip. Build echoes are always folded. Earlier checkpoint
  sections are collapsed.
- The empty-state legend teaches what really renders: the evidence strip in place of the dead
  tool-call row, and "reasoning" without "editable".

## 10. Slice chosen for this pass, and why

**The turn evidence strip, a neutral turn-ended glyph, and tail-follow scrolling.** These
three fixes are the top three of the audit's ranking.

- They make the thesis visible at a glance (one tick or cross per certificate at the end of
  every turn), and they name the "built but uncertified" case.
- They work on the **live** path today: certificates already arrive as fences on the
  claude-code provider, while tool frames do not.
- They are frontend-only, and their pure logic (`lib/turn-evidence.ts`,
  `lib/blackboard-scroll.ts`) is testable under `node:test`.
- §2, §3 (buttons), §4, §6 and §7's move to the status bar are the next slices, in that order.
