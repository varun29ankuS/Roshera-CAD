# The human ↔ agent feedback loop: design

2026-11-17. Companion to `PARITY_AND_FEEDBACK_AUDIT.md`. Builds on the 11-13 timeline decisions (plan = a movie; picking both ways; three views of one history).

**The loop.** While an agent works, the person points, hovers, selects, corrects a pick, accepts or rejects a step — and the agent knows at once. Whatever the agent built, the person edits with the same dialogs as their own work.

Four parts, in build order. Slice 1 (the first half of §a) is implemented in this change; the rest is design.

| | Part | State |
|---|---|---|
| a | Live user context: the agent can read what the person points at | **slice 1 built**; push + message attachment designed |
| b | Pick mode both ways | designed |
| c | Accept / reject / edit at step k | designed |
| d | The parity rule | designed; two one-line fixes ready |

Rules that hold throughout:

- **Persistent ids, never re-targeting.** An entity travels by its kernel id and its persistent id. One that no longer resolves is returned as unresolved with the reason. Nothing is ever swapped for a "nearest match".
- **The viewport is the work.** Everything below is a status-bar item, a banner over the canvas, or a card on an existing overlay. No new docked panel.
- **Tick/cross, not prose.** Accept and reject are buttons on a card, not sentences in a chat.

---

## a. The live user context

### What is shared

Entities, not mouse movement.

| Shared | Not shared, ever |
|---|---|
| The part / face under the cursor (kernel id, persistent id, the point on it) | Screen pixels, cursor trail, keystrokes |
| The selection: parts, faces, edges, vertices | Anything outside the viewport and the history |
| The feature selected in the history; the step being viewed | The Blackboard draft |
| Active tool, pick filter, workspace | |
| Camera: position, direction, up, projection, named view | |

Defaults, chosen to respect the person:

- **Sharing is on, visibly.** The status bar always says what the agent would be told: `Agent sees: Flange · face 56`. (The app already sent every click to `/api/agent/pointer` with no indication; this replaces a silent channel with a visible one.) *Decision for Varun: on by default, or off until the first agent session starts?*
- **One click pauses.** Paused, the server is told only "paused", stores nothing else, and tells agents to ask the person.
- **Nothing is history.** Not written to the timeline, not written to disk; the server keeps one report per document and forgets it when the next arrives.
- **The person sees the agent look.** Each agent read lights the eye: `· Claude is looking`.

### Status-bar states

```
Pick [Part] Face Edge Vertex │ Snap off │ X 12.40 Y −3.00 Z 20.01 │ Under cursor: Flange · face 56 │ 👁 Agent sees: Flange · face 56            3 parts │ ✓ sound
                                                                                                    👁 Agent sees: Flange · face 56 · Claude is looking     (eye pulses, 4 s)
                                                                                                    👁 Agent sees: selected Flange · face 3
                                                                                                    👁 Agent sees: 4 selected
                                                                                                    👁 Agent sees: Flange · an edge                         (edge filter: no id until picked)
                                                                                                    👁 Agent sees: nothing pointed at
                                                                                                    ⦸ Agent sees: nothing · paused                          (muted)
```

The label is what the server last acknowledged, so it is what an agent would actually read, not what the app intends to send. The tooltip adds `Claude looked 12 s ago`.

### API (built)

`POST /api/agent/user-context` — the app reports. Latest wins per document.

```json
{ "sharing": true, "workspace": "part", "tool": "select", "pick_filter": "face",
  "hover": { "object_id": "<part uuid>", "kind": "face", "id": 56, "point": [12.4, -3.0, 20.01] },
  "selection": [ { "object_id": "<part uuid>", "kind": "face", "id": 3 } ],
  "feature": 7, "history_step": null,
  "camera": { "position": [..], "direction": [..], "up": [..], "projection": "perspective", "preset": "isometric" },
  "at_ms": 1700000000000 }
→ { "seq": 18, "agent_look": { "agent": "Claude", "age_ms": 1200, "seq": 17 } }
```

- Unknown fields, more than 256 entities, non-finite numbers: refused whole, `invalid_parameter`, with the parameter named.
- `sharing: false` stores the pause and drops everything else in the body.

`GET /api/agent/user-context` — an agent reads. Always 200.

```json
{ "state": "live", "document_id": "...", "seq": 18, "age_ms": 140,
  "workspace": "part", "tool": "select", "pick_filter": "face",
  "hover": { "kind": "face", "object_id": "...", "id": 56, "point": [..],
             "resolved": true, "solid_id": 3, "part_name": "Flange",
             "part_persistent_id": "…32 hex…", "persistent_id": "…32 hex…",
             "surface_type": "cylinder", "area": 502.6 },
  "selection": [ … ], "selection_total": 1,
  "feature_anchor": 7, "history_step": null, "camera": { … } }
```

| `state` | Meaning | What the agent should do |
|---|---|---|
| `live` | reported within 6 s | use it |
| `stale` | the app stopped reporting (closed, or in the background); content is the last it said, `age_ms` old | treat as a hint; confirm |
| `paused` | the person paused sharing; no entities | ask the person |
| `none` | no app has reported for this document | ask, or use `guide_attention` to open it |

An entity that does not resolve:

```json
{ "kind": "face", "object_id": "...", "id": 987654, "resolved": false,
  "reason": "face 987654 is not in the model: a later operation replaced it" }
```

Other reasons: not a face of this part; the part was deleted or consumed; not a part of this document (an assembly instance); the app has no kernel id for this edge (only the part and point are known); the document is not the one open in the kernel.

A read carrying `X-Roshera-Agent` is an agent looking: it is noted, returned in the next report's answer, and broadcast as one frame:

```json
{ "type": "AgentLooked", "payload": { "agent": "Claude", "document_id": "...", "seq": 18, "saw": "live" } }
```

MCP: tool `user_context` (core bench, read-only, no arguments). `get_pointer` stays as it is for now.

### When the app sends

| Trigger | Sent |
|---|---|
| Hover, selection, feature, step, tool, filter or workspace changes | once the context has been still for 150 ms; never later than 1 s after the first unsent change |
| Camera moves only | nothing; the camera rides on the next report or heartbeat |
| Nothing changes | a heartbeat every 2 s while the tab is in front |
| Tab in the background | nothing (the server calls it `stale` after 6 s) |
| Pause | the pause, once; no heartbeats |
| A request is in flight | the newest report waits; the ones between are dropped |

Latency, person → agent: at most ~150 ms to the server after the cursor rests, plus the agent's next tool call. The second term is the one that matters, which is why push comes next.

### Next in §a (not built)

**Push: MCP resource `roshera://user-context`.**

- `resources/subscribe` on it; the MCP server holds one SSE connection to a new `GET /api/agent/user-context/events` and emits `notifications/resources/updated` when `seq` advances *and* the change is meaningful: selection changed, feature changed, a pick was answered, a step verdict was given. Hover alone does not notify.
- Clients that do not support resource subscription keep the pull tool. The harness can also fold a one-line context summary into every tool result (as it already folds certificates), so an agent that never asks still sees it: `user: selected Flange · face 3 (pid 4f…)`.

**With the message.** When the person sends a Blackboard message, the app attaches the resolved context as it was at send time:

```json
{ "text": "make this one 2 mm bigger",
  "context": { "seq": 18, "selection": [ { "kind": "face", "id": 3, "persistent_id": "…", "part_name": "Flange", … } ],
               "hover": null, "feature_anchor": 7 } }
```

- The composer shows what will travel, as chips above the input: `[Flange · face 3 ✕] [feature: Bore ✕]`. Removing a chip removes it from this message only.
- It reaches the agent as a structured block in the prompt, not as prose. The Blackboard line keeps the chips, so the record shows what "this" meant.
- Fetch through `GET /api/agent/user-context` without the agent header (no "looking" light for the app's own read).

---

## b. Pick mode both ways

### Agent asks the person to pick

`POST /api/agent/pick-request` (MCP `ask_pick`):

```json
{ "prompt": "Which face should the bracket sit on?", "filter": "face", "count": 1,
  "part": 3, "candidates": [ { "face_id": 12 }, { "face_id": 40 } ], "timeout_s": 120 }
→ { "request_id": "…" }        (broadcast: { "type": "PickRequest", … })
```

The app enters pick mode:

```
┌───────────────────────────────────────────────────────────────────────────┐
│ ◉ Claude asks: Which face should the bracket sit on?   Face · 0 of 1   [Skip] [✕ Cancel] │   banner, top of canvas
└───────────────────────────────────────────────────────────────────────────┘
        candidates glow amber; everything else dims to 60 %; the pick filter is locked to Face;
        hover shows the usual highlight; a click on a candidate answers; a click elsewhere on the part
        answers too, marked "not one of the suggestions"
```

| State | Canvas | Banner | Exit |
|---|---|---|---|
| waiting | candidates amber, filter locked | prompt, `0 of N` | pick, Skip, Cancel, Esc, timeout |
| picked (count > 1) | picked ones solid, counter advances | `2 of 3` + `[Done]` | Done |
| answered | flash, then normal | gone | — |
| timed out / cancelled | normal | gone | the agent gets `cancelled` / `timed_out`, never a guess |

Answer: `POST /api/agent/pick-request/{id}/answer`, and the agent's call returns:

```json
{ "status": "picked", "picks": [ { "kind": "face", "id": 40, "persistent_id": "…", "solid_id": 3,
                                    "point": [..], "was_candidate": true } ] }
```

The agent's `ask_pick` blocks (long-poll, like `ask_choice`) until answered, cancelled or timed out.

### Agent proposes, the person corrects

`guide_attention` already lights faces. Add `proposal: true`:

```
┌───────────────────────────────────────────────────────────────┐
│ ◉ Claude will fillet these 4 edges, R3        [✓ Use these] [✎ Change] [✕ No] │
└───────────────────────────────────────────────────────────────┘
```

- **✓** answers with the proposed set.
- **✎** drops into pick mode with the proposal pre-selected; click adds or removes; `[Done]` answers with the corrected set and a diff (`added`, `removed`), so the agent learns what it got wrong.
- **✕** answers `rejected`.

### In the timeline

A pick that feeds an operation is part of that operation's recipe: the op is recorded with the picked persistent ids (as `fillet_edges` already records `edge_pids`) and with `picked_by: user | agent | agent_corrected_by_user`. The request itself is not an event. On replay a pick whose persistent id is gone is a refusal, never a re-target.

---

## c. Accept / reject / edit at step k

### Flow

1. The agent submits a plan: `POST /api/agent/plan` with a recipe (the same step list `cad_program` takes) and an intent line.
2. The kernel replays it on a **scratch branch** (branches exist: `POST /api/branches`). Each step gets its frame picture and certificate. The result is deterministic, so the preview is the result.
3. The app shows the plan in the film strip, as a second row above the committed history:

```
 plan  ◌ Claude: "M6 mounting holes on the base"                              [▶ Play] [✓ Accept all] [✕ Reject]
       ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐
       │ img  │ │ img  │ │ img  │ │ img  │ │ img  │      amber border = proposed, not yet in the part
       │Sketch│ │Extrud│ │ Hole │ │Pattrn│ │Fillet│
       │  ✓   │ │  ✓   │ │  ✓   │ │  ✓   │ │  ✗   │      ✓/✗ = the kernel's certificate for that step
       └──────┘ └──────┘ └──────┘ └──┬───┘ └──────┘
                                      └─ selected step: [✓ Accept to here] [✎ Edit] [✕ Reject from here]
 main  ━━●━━━●━━━●━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  slider (the only time travel)
```

4. The person's verdict:

| Action | Effect | The agent receives |
|---|---|---|
| ✓ Accept all | scratch branch merges (fast-forward) | `accepted`, through step n |
| ✓ Accept to here (k) | steps 1..k merge; k+1.. are dropped | `accepted_through: k`, `rejected_from: k+1` |
| ✕ Reject from here (k) | steps 1..k-1 merge if the person says so, else nothing | `rejected_from: k`, optional reason |
| ✎ Edit (k) | the same `FeatureEditPanel` opens on step k of the scratch branch; the change re-shoots k..n there | `edited: {step: k, parameter, from, to}` + the new certificates |
| ✕ Reject | the branch is abandoned (`DELETE /api/branches/{id}` — the route exists, with no caller today) | `rejected`, optional reason |

A reason is one optional line, typed or picked from chips (`wrong face`, `too big`, `not what I asked`). With a pick attached — the person clicks the face they meant — it carries the persistent id.

### Events

Recorded on main, so the history shows who decided what:

| Event | Payload |
|---|---|
| `plan.proposed` | plan id, agent, intent, recipe hash, scratch branch |
| `plan.accepted` | plan id, through step k, by whom |
| `plan.rejected` | plan id, from step k, reason, optional pick |
| `plan.edited` | plan id, step k, parameter, from → to |

These are decision records (they show on the timeline's decision rail), not geometry; replay skips them the way it skips checkpoints. The accepted steps themselves are ordinary events authored by the agent.

### Live build without a plan

An agent that builds step by step (no plan) still gets a verdict per step: each new agent chapter shows `[✓] [✕]` on its tree row for as long as it is the newest. ✕ is today's delete (suppress, trialled) plus a `plan.rejected`-shaped message to the agent, so the agent learns the step was removed and why rather than discovering it on its next read.

---

## d. The parity rule

> **Anything an agent can build, a person can select, open and change with the same dialog, and undo — and the dialog offers every parameter the agent set.**

Stated as checks, so it can be a gate (a test over `agent_registry.rs` × `chapters.rs` × the app's edit tables, like the ontology drift gate):

1. Every mutating MCP tool records a step whose chapter kind the tree names and has an icon for. *Fails today:* `psketch_revolve` → `sketch_revolve`, `nurbs_loft`, `import_step`, GD&T, colour, labels (not recorded at all).
2. Every parameter in the tool's schema is either editable in `FeatureEditPanel` or listed as fixed with a reason. *Fails today:* edges, operands, counts, axes, transforms, mate values and limits.
3. A feature made from several steps undoes as one. *Fails today:* holes, patterns, `cad_program`.
4. Agent edits to a document appear in the open app without a reload. *Fails today:* assemblies.
5. Every operation the agent has, the person has a button for — or the gap is on a list. *Fails today:* hole, pattern of holes, mate limits/overlays/couplings, instance move, GD&T.

Order of work, cheapest first:

| # | Fix | Size |
|---|---|---|
| 1 | `roshera-mcp/src/tools/assembly.ts:354`: `conn.id` → `conn.connector_id` (agents cannot create mates today) | one line + a stub-server test |
| 2 | `timeline-engine/src/chapters.rs:509`: `sketch_revolve` is a revolve (gives it the icon and Edit sketch) | one line + a chapter test |
| 3 | The edit panel stops offering ids and counts as sizes (`face_id`, `datum_id`, `segments` …) | allow-list per command |
| 4 | Assembly live: broadcast an `AssemblyChanged` frame from the assembly handlers; the store reloads on it and on `HISTORY_CHANGED_EVENT` | small |
| 5 | Typed edit fields beyond numbers: **re-pick** (edges of a fillet, faces of a shell) through pick mode §b; **choice** (boolean type); **count** — needs `drill_pattern` and the body patterns to record a real pattern step (count, pitch, axis) instead of N copies | medium; the pattern step is a kernel-side recording change |
| 6 | Mate edit card: value, limits, flip; instance move gizmo and pose fields; timeline in the Assembly workspace | medium |
| 7 | Feature-level undo: undo walks back a whole chapter | medium |
| 8 | GD&T edit UI (already queued: "GD&T is design history") | large |

---

## The first slice (built in this change)

**What:** §a's pull half, end to end — the app publishes hover, selection, feature, step, tool, filter, workspace and camera; the server keeps the latest per document and resolves entities to persistent ids at read time; the agent reads it with `user_context`; the status bar shows `Agent sees: …`, lights when an agent reads, and pauses on a click.

**Why this one first:**

- Every other part needs it. Pick mode answers, step verdicts and message attachments all travel as "an entity by persistent id, resolved or refused" — this slice builds that resolver and the per-document store once.
- It is safe to ship alone: it records nothing, changes no geometry, and the person can see and stop it.
- It replaces a channel that was silent, global and click-only.

**What it deliberately leaves out:** push, message attachment, assembly instances and mates, sketch entities, drawing dimensions, hovered edges/vertices by id, per-user scope. Each is listed above or in the audit's "still missing".
