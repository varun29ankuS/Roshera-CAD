# Conversation UI audit: the Blackboard (2026-11-18)

> **HISTORICAL (2026-11-22).** Its top-ranked fixes shipped as the design's first slice (6987cac3,
> every agent turn ends with the kernel's verdict); card-first replies 7d7b2ff5; the user's
> selection travels with the message a4d5258a. Next slices are in `CONVERSATION_UI_DESIGN.md` §10.

Scope: the agent conversation panel in `roshera-app/`. That is `components/panels/Blackboard.tsx`
(the panel and composer), `BlackboardSection.tsx` (checkpoint sections), `BlackboardLine.tsx`
(which shape a line gets), `BlackboardMessageParts.tsx` (tool row, failed turn, long prose),
`BuildStepStrip.tsx`, `TurnStatusLine.tsx`, `PermissionCard.tsx`, and `cards/*`. The data
comes from `stores/blackboard-store.ts`, `lib/acp-blackboard.ts` (ACP turn to lines),
`lib/ws-bridge.ts` ("Created …" echoes, "Server refused: …"), `lib/agent-activity.ts`
(`GET /api/acp/activity`) and `lib/blackboard-api.ts` (backend persistence).

Method: I read the code only. No browser was available, so no screenshot was taken. Every
visual claim below comes from class names and JSX. Claims marked **[inferred]** are about
behaviour I reasoned from the code without observing it.

## 0. What actually reaches the panel on the live path

This matters more than any styling. On the default provider (goose's `claude-code` ACP
bridge), `tool_call` and `tool_call_update` frames never arrive. The tools run inside the CLI
subprocess. This is documented, measured live, in `acp-blackboard.ts` and in
`blackboard-store.ts`'s `AgentAttention` doc. In a real turn, the board therefore receives:

| Arrives | As | Source |
|---|---|---|
| the user's ask | `user` line | `ai-client.ts processBlackboardMessage` |
| streamed agent text | one `agent` line, edited in place | `agent_message_chunk` / `agent_thought_chunk` |
| certificates, DFM, FCF, refusals, choices | `roshera:*` fences inside agent lines (streamed text, or separate lines the agent posts with `blackboard_add_entry`) | `roshera-agent-policy.md` §"Forward verdicts verbatim" |
| per-solid echo `Created **x** — … · N tris` | `system` line, which round-trips as `agent` | `ws-bridge.ts` flush |
| `Server refused: …` | `system` line | `ws-bridge.ts` `Error` frame |
| what is running now | `TurnStatusLine` only, polled every 5 s | `/api/acp/activity` `recent_operations` |
| permission asks | `PermissionCard` at the tail | ACP `session/request_permission` |
| plan entries | **dropped** (`default: return // plan … not yet surfaced`) | `acp-blackboard.ts` |

`ToolCallRow` is therefore dead on the live path. The per-operation record of a turn exists
only while the turn runs (`TurnStatusLine`). It is not written to the notebook, so once the
turn ends the conversation keeps no record of which kernel operations the agent ran.

## 1. Per message/part kind: what it shows today

| Kind | Rendering today | Hard to read / missing |
|---|---|---|
| **User text** | Indented 24 px, medium weight, primary-tinted rounded box, `User` marker. | Clear. Missing: what the user had selected when they asked. The live user-context channel (`user-context-store`) exists, but nothing about it is attached to the message. |
| **Agent text** | Flush prose, 13 px/1.6, measure 72ch, capped at 320 px with a "Show full message" button. Chalk reveal on arrival. | Readable. Long multi-step turns are one undivided paragraph block. Intent, step and result are not separated **[inferred]**. |
| **Tool call** | `⚙ title — status` parsed into a mono name, a status chip (queued/running/done/failed) and a "result" toggle. | **Never fires on the live provider.** No parameters and no duration (the transport carries neither). The empty-state legend still teaches "tool calls — one row each", which a user will never see. |
| **Tool result** | Behind the "result" toggle. It renders only when the payload validates against a card schema. | Same as above: dead path. |
| **Certificate / verdict** | Typed cards (`SoundnessCard`, `DfmCard`, `FcfCard`, `MergeCard`) render inside the line. The line's marker circle takes the verdict colour (emerald/red/amber). | Each card is good on its own. **The turn has no verdict.** To learn whether a 6-step turn ended sound, the engineer scrolls the turn and reads every card. The colour on the 16 px marker is the only at-a-glance channel, and it is per line, not per turn. A turn that changed geometry and posted **no** certificate looks exactly like a certified one. |
| **Turn outcome glyph** | `completed` = **emerald tick**, failed = red cross, cancelled = amber slash, set from `turnStatus`. | **Undermines the thesis.** The emerald tick next to the reply says "turn ended", but at a glance it reads as "sound". It uses the same hue and glyph as a proven certificate (the store's doc warns about exactly this). `turnStatus` is also **not persisted** (`blackboard-api.ts` never writes it), so after a reload the glyph disappears and an identical history reads differently. |
| **Refusal** | Typed card when it arrives as a `roshera:refusal` fence: sky accent, verbatim reason, `error_code` chip, retryable word, hint, "next" options. | Good when typed. A WS-level refusal arrives as a plain `Server refused: …` system sentence with no card, code or suggested fix. A refusal turn on the ACP error path renders through `FailedTurnBlock`, red, which contradicts "a refusal is a result, not an error" whenever it is not fenced. |
| **Choice card** | `ChoicesCard` buttons with sky rule. `DetectedChoicesCard` adds buttons under a prose "Option A: …". The answer is written back into the line. | Good. Missing: plan-level accept/reject. ACP `plan` frames are dropped, so the user cannot approve a plan step by step. |
| **Error (turn failed)** | `FailedTurnBlock`: red bordered block with "Turn failed" and the verbatim message. | Clear. Some messages are long paragraphs (provider failure) rather than a typed reason plus a fix line. |
| **Long-running op** | `TurnStatusLine`: a breathing star, a rotating shop-floor verb, `(elapsed · tokens · N ops)`, `⎿ current activity`, and Stop. Header chip "writing" / "executing". | It is the only live progress, and it is good. But it **puts numbers in the panel** (elapsed, tokens, op count), which breaks "numbers live in the status bar". The operation list vanishes when the turn ends. `geometry` attention never fires on the live path, so "executing — viewport has focus" never shows. |
| **Build-step echoes** | Runs of two or more "Created …" lines fold into `BuildStepStrip` (`bore ×4 · Difference — 5 steps`). | Good compaction. The `— N steps` count is a number in the panel. Not linked to a certificate. |
| **Image / render** | **None.** `MessageMarkdown` has no image component. `render_part` / `scene_view` output never reaches the board. | An engineer cannot see what the agent saw. This is a hard gap for "the agent sees and acts deterministically". |
| **Permission ask** | `PermissionCard` at the tail, plus `PermissionStrip` in the composer. | Good. |
| **Queued prompt** | A dashed amber strip above the composer, "N queued". | Good. The user line is added immediately, so two asks can sit adjacent and turn 1's work lands under ask 2 **[inferred from `processBlackboardMessage`]**. |
| **Checkpoint sections** | Sticky header per checkpoint. Earlier ones are collapsed, with a line count. | The agent declares a checkpoint **mid-turn**, so the user's ask lands in the previous section and its work in the next one (`groupBlackboardByCheckpoint` buckets by timestamp). A turn is split across a collapsed header **[inferred]**. |

## 2. Cross-cutting defects found in the code

1. **Autoscroll yanks the reader.** `Blackboard.tsx` sets `scrollTop = scrollHeight` on every
   change to `lines`, `agentAttention`, `overrideHeight` or `isProcessing`. Anyone scrolled up to
   read a certificate is thrown to the bottom by the next "Created …" echo or streamed chunk.
   There is no "jump to latest".
2. **11 px floor broken.** The composer hint "Enter sends · Shift+Enter new line" is
   `text-[10.5px]`.
3. **Empty-state legend is wrong.** It says "the agent — reasoning, editable", but the
   notebook has been read-only since 2026-08-01.
4. **System authorship is lost on round-trip.** System lines persist as `agent`
   (`blackboard-groups.ts` doc). After a reload, kernel bookkeeping wears the Bot marker.
   This needs a backend fix (a `System` author on the wire).
5. **The panel is `bg-card/95`, an opaque card, not a transparent overlay.** It is closable and
   there is no blur, but at 95 % opacity it reads as a solid panel over the viewport
   **[inferred]**.

## 3. Ranked by impact on an engineer following the agent

1. **No per-turn verdict** (§1 certificate row). The whole thesis, "the kernel cannot lie",
   is invisible at a glance. It needs scrolling and reading. A turn that built geometry and
   posted no certificate is indistinguishable from a certified one.
2. **The "turn completed" emerald tick looks like "sound"**, and it is not persisted.
3. **Autoscroll yanks the reader off the evidence** (§2.1).
4. **Tool operations leave no record after the turn** (dead `ToolCallRow`; ops visible only
   live). This needs a backend change; see the design doc's backend asks.
5. **No images/renders in the conversation.**
6. **The user's selection is not attached to the ask.** The agent reads it live, but the
   record does not say what "this face" meant.
7. **Turns are split by mid-turn checkpoints and by queued adjacent asks.**
8. **Untyped refusals** (`Server refused: …`, red failed-turn blocks for refusals).
9. **Plan frames dropped.** No step accept/reject.
10. **Numbers in the panel** (TurnStatusLine elapsed/tokens/ops, strip step counts).
11. **Small fixes:** 10.5 px hint, wrong "editable" legend, a legend entry for tool rows that
    never render.
