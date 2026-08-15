/**
 * Dispatch gates — the CONSTRAINT layer at the ToolTable choke point
 * (2026-08-01 policy-constraint audit, items §5/§9/§10 → structure).
 *
 * Every harness layer either asks (steering — the model may decline) or
 * prevents (constraint — the wrong thing is inexpressible). Measured on this
 * project, constraints held identically across models while steering degraded,
 * so three policy sentences are converted here into refusals the retry loop
 * cannot argue with. All three ride the ONE choke point every call path runs
 * through — direct mount, invoke(), and cad_program()'s batch loop all call
 * the wrapped handler ToolTable.add() builds (registry.ts), so nothing is
 * gated twice and nothing escapes the gate.
 *
 *   1. REFUSAL CACHE (audit §10). A typed refusal is a stable fact, not a
 *      retry target. The first refusal for `(tool, canonicalJson(args))` is
 *      remembered; an IDENTICAL re-issue is answered with the SAME refusal
 *      from cache, never re-hitting the kernel, until some state-changing
 *      call succeeds (at which point the world may genuinely have changed and
 *      the whole cache is dropped — a re-issue then re-earns its answer).
 *      Changed args always pass through. Per-session (this process), bounded.
 *
 *   2. INTENT GATE (audit §5). A solid-mutating call with no open intent
 *      checkpoint is refused, with the exact call that opens one named in the
 *      refusal. This gate is UNCONDITIONAL — refused always, not refused-once
 *      — because unlike the unsound-base gate there is no legitimate flow
 *      that needs to bypass it: opening a checkpoint is one cheap call that
 *      is never impossible, so an escape hatch would only reintroduce the
 *      steering this gate replaces. Generic sequence-position names
 *      ("step 3", "cp 2") are refused too, or the gate would be satisfied by
 *      exactly the names the policy exists to prevent.
 *
 *   3. UNSOUND-BASE GATE (audit §9). A mutating op whose base solid's live
 *      kernel verdict is sound==false is refused unless the caller passes
 *      acknowledge_unsound:true (deliberate repair flows: booleans used to
 *      heal, rollback ops). These refusals are NEVER served from the refusal
 *      cache: the underlying fact is live kernel state that another author
 *      (the human, another agent) can change without a call from this
 *      session, so every re-issue re-reads the live verdict — still refused
 *      while unsound, allowed the moment it is repaired, no matter who
 *      repaired it. The gate is a pre-flight guard, not the certification:
 *      when the verdict cannot be fetched (timeout, unreachable backend) the
 *      op proceeds and its own ambient certificate still tells the truth —
 *      proceeding without a pre-flight fact is not an approximation, because
 *      nothing is asserted; refusing on a transport hiccup would be.
 *      `make_drawing` rides this gate too: a sheet projected from a defective
 *      solid prints that defect as dimensioned truth, and the only honest
 *      exception (an inspection sheet OF the defect) is exactly what
 *      acknowledge_unsound exists to express.
 *
 *   4. SHEET-EXPORT GATE (2026-08-01 drawing-harness pass). Export is the one
 *      moment a drawing stops being live data and becomes a shop artifact —
 *      a PDF/DXF on disk carries NO ambient certificate, so unlike a kernel
 *      op there is no downstream truth-teller after this point. Before
 *      `drawing_export_sheet` runs, the sheet's LIVE certificate is read:
 *        - stale or dangling facts (the model moved since projection, or a
 *          referenced face is gone) → refused, NO bypass — regenerating with
 *          make_drawing is one cheap call that is never impossible, and a
 *          sheet whose printed dimensions disagree with the model must never
 *          reach a shop;
 *        - layout-quality Errors (label collisions, redundant dims — the
 *          findings a checker rejects on sight) → refused unless
 *          acknowledge_layout_issues:true (the draft-for-human-review flow:
 *          sometimes the defective layout is exactly what a human asked to
 *          see);
 *        - certificate unreadable → refused. This inverts gate 3's
 *          fail-open: gate 3 may proceed because the op's own certificate
 *          still tells the truth afterwards; an exported file asserts every
 *          printed dimension and can never re-verify itself, so exporting
 *          uncertified WOULD be an approximation labeled as exact.
 *      All three are live facts and are never served from the refusal cache.
 *
 *   5. SINGLE-POINT-RUN GATE (2026-08-09 token-burn constraint; cumulative
 *      half added 2026-08-15, audit S11/item 9). Measured failure: an agent
 *      laid out a 256-point gear profile one psketch_add_entity
 *      {kind:'point'} call at a time — 1.3M tokens for geometry ONE
 *      polyline call expresses. The tool descriptions already steer against
 *      it ("NEVER loop this tool"); steering is ignorable, so this gate is
 *      the constraint: after SINGLE_POINT_RUN_MAX consecutive successful
 *      single-point additions to the SAME sketch with no other call in
 *      between, the next one is refused typed, naming the count and the
 *      bulk path (psketch_add_entity kind:'polyline' — every vertex in ONE
 *      call). ANY other tool call resets the RUN counter, so a legitimate
 *      small sketch (2-8 named vertices for constraints) never meets the
 *      gate.
 *
 *      The run counter alone is defeated by ONE filler call per burst:
 *      `psketch_add_entity{point}×8 → list_parts{} → repeat` never trips it
 *      while still reaching the 256-point failure mode at the cost of one
 *      cheap call per 8 points — acknowledged in the run counter's own
 *      design as the price of never bothering a legitimate small sketch,
 *      which is the right trade for a SAFETY RAIL but the wrong one for a
 *      TRAINING SIGNAL: it teaches "insert a filler call", not "use the
 *      bulk path". A SEPARATE cumulative counter closes that: it counts
 *      every successful single-point addition to a sketch across the WHOLE
 *      session and is NEVER reset by an intervening call — not by a filler
 *      call (the whole point) and not even by a polyline call to the SAME
 *      sketch (the round-trip cost already spent placing the points already
 *      placed is not undone by later bulk work; the cumulative total is a
 *      permanent fact about this sketch's history). Both counters ride the
 *      same live-fact discipline (session state, never cached) but trip
 *      under DIFFERENT typed gate names (`single_point_run` /
 *      `single_point_cumulative`) so a trajectory can tell which condition
 *      fired. Purely MCP-side session state — no backend change.
 *
 *   6. VERIFICATION-SCOPE GATE (2026-08-11, task #9 half B). Gate 2 forces an
 *      intent to be DECLARED before geometry is built. Nothing forced anyone
 *      to look at what came out. Measured elsewhere in this repo: a loft
 *      shipped CERTIFIED SOUND at a 9.97% shape error, because soundness is a
 *      statement about topology and says nothing about whether the result is
 *      the geometry that was asked for — so "check what you built" cannot be
 *      left to steering either. When a checkpoint CLOSES (a new
 *      timeline_checkpoint replaces the open one — the only close this surface
 *      has) and solid-mutating ops ran under it with NO verify_part /
 *      verify_claim since the last of them, the closing call is refused typed,
 *      naming exactly what was built and the two verification verbs. ONE
 *      escape, and it is explicit: `skip_verification: true` on the closing
 *      checkpoint. The constraint is therefore escapable but NEVER silent —
 *      the escape is a recorded argument in the call, not an omission.
 *      `clear_timeline` is deliberately OUT of scope: it wipes the ledger the
 *      work lives in, so nagging to verify a part whose history is being
 *      destroyed is noise, not a constraint. Live session state, never cached
 *      (a verify_part between attempts must unblock the very next re-issue of
 *      the IDENTICAL checkpoint call — a cached refusal would deadlock exactly
 *      the caller who complied).
 */

import { api, PERCEPTION_TIMEOUT_MS } from "./core.js";
import { canonicalJson, fnv1a64hex } from "./registry.js";

// ─── Typed refusal shape ────────────────────────────────────────────────────

/**
 * Mint a gate refusal as a tool result. `refused: true` is the machine-typed
 * marker (same convention kb_lookup and the timeline refusal path use);
 * `isError: true` makes cad_program stop its batch at the refused op and
 * makes the funnel surface it as a failure, because a refused mutating op
 * produced no geometry and everything after it would build on a lie.
 */
function gateRefusal(payload: Record<string, unknown>) {
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({ refused: true, ...payload }, null, 2),
      },
    ],
    isError: true as const,
  };
}

/**
 * Detect a typed refusal in ANY tool result, whatever path produced it:
 *  - a JSON payload whose top-level `refused` is `true` (kb_lookup, the gates
 *    here) or an object (the timeline tools' `ok({refused: <backend body>})`),
 *  - an error result whose text carries the kernel's REFUSED marker
 *    (drill_pattern's spacing guard, backend typed refusals).
 * Returns the parsed gate name when one exists (cache policy needs it), an
 * empty object for a refusal with no gate, or null for a non-refusal.
 */
function typedRefusalOf(result: any): { gate?: string } | null {
  const content: any[] = Array.isArray(result?.content) ? result.content : [];
  const first = content.find(
    (c) => c?.type === "text" && typeof c.text === "string",
  );
  if (!first) return null;
  const text: string = first.text;
  try {
    const data = JSON.parse(text);
    if (data && typeof data === "object") {
      const r = (data as any).refused;
      if (r === true || (r !== null && typeof r === "object")) {
        const gate = (data as any).gate;
        return { gate: typeof gate === "string" ? gate : undefined };
      }
    }
  } catch {
    // not JSON — fall through to the marker check
  }
  if (result?.isError === true && /\bREFUSED\b/.test(text)) return {};
  return null;
}

// ─── Tool classification ────────────────────────────────────────────────────

/**
 * Tools that cannot change kernel/session state — their success never
 * invalidates a cached refusal. Everything NOT listed here is treated as
 * state-changing (the safe default: an unlisted tool can only make the cache
 * FORGET sooner, never serve stale). The meta/composition tools delegate to
 * inner handlers that pass through this same gate individually, so the outer
 * call itself is classified read-only.
 */
const READ_ONLY = new Set<string>([
  "list_parts",
  "get_part",
  "render_part",
  "scene_view",
  "section_view",
  "verify_part",
  "mass_properties",
  "get_pointer",
  "select_face",
  "select_edge",
  "get_face",
  "get_revolve_profile",
  "plane_from_face",
  "point_query",
  "ray_query",
  "region_query",
  "occupancy_view",
  "part_coverage",
  "part_distance",
  "part_features",
  "dfm_check",
  "kb_lookup",
  "ground_truth",
  "measure_faces",
  "verify_claim",
  "timeline_history",
  // A pure GET projection over a recorded log — it opens no document and
  // touches no model, so a recipe retrieval can never make a cached refusal
  // stale.
  "recipe_get",
  "timeline_branches",
  "timeline_conflicts",
  "timeline_checkpoints",
  "timeline_scrub",
  "rebuild_certificate",
  "label_list",
  "label_resolve",
  "blackboard_list",
  "assembly_list_instances",
  "assembly_view",
  "assembly_dof",
  "assembly_interference",
  "assembly_verify",
  "drawing_query",
  "drawing_read_semantics",
  "gdt_report",
  "export_part",
  "drawing_export_sheet",
  "find_tool",
  "describe_tool",
  "invoke",
  "workbench",
  "cad_program",
]);

/**
 * Solid-mutating verbs the intent gate covers: everything that creates,
 * reshapes, or destroys a solid — the calls the policy's "checkpoint before
 * the first mutating call of every feature" was written about. Sketch
 * construction (psketch_begin/add_entity/constrain/solve) is deliberately
 * NOT gated: the feature materialises at the extrude/revolve, and nagging
 * during sketch iteration would teach the model to open garbage checkpoints.
 * Assembly and label tools are out of scope here (assembly placement policy
 * is mid-flight in the audit-fix wave, audit §6).
 *
 * `timeline_mould`, `delete_part`, `clear_parts` (audit S6/S7): all three
 * change the live model and were previously invisible to this set.
 * `timeline_mould` "edits a recorded parameter and re-derives the model" —
 * geometry changes exactly as a create/reshape op's does, and its absence
 * here was the audit's own exploit path (checkpoint → build → verify_part →
 * mould the just-verified thing → close the NEXT checkpoint, which used to
 * pass reporting the mould's target as verified when it is not — see gate 6's
 * `intentUnverified` bookkeeping below, which keys off THIS set, so adding a
 * tool here both requires an intent to run it AND arms gate 6 for it, closing
 * both halves of S6/S7 with one list membership). `delete_part`/`clear_parts`
 * change the live model with no gate of their own: `cad_program` guards them
 * behind `allow_destructive`, but only for ops it validates and dispatches
 * itself — a direct call or `invoke` reached neither that guard nor this one.
 * The `BASE_REFS` half of `timeline_mould` (a pre-flight unsound-base check)
 * is deliberately NOT added: a mould targets a recorded EVENT, not a live
 * solid uuid/part_id, and `BASE_REFS`'s extractors assume the latter — left
 * out per the brief rather than forcing a mismatched shape.
 *
 * Deliberately NOT gated, and this is the actual decision the audit found
 * undocumented (S7): `timeline_undo`, `timeline_redo`, `timeline_switch`,
 * `timeline_merge`. These are history NAVIGATION, not new feature work —
 * requiring a declared intent to undo a mistake would mean opening a
 * checkpoint in order to fix a checkpoint, which defeats the point of undo
 * as a low-friction correction. `clear_timeline` is excluded from gate 6 for
 * the same reason (see its own module-doc note); it is not in this set
 * either, since it destroys the branch's events rather than reshaping a
 * solid.
 */
const MUTATES_SOLIDS = new Set<string>([
  "create_box",
  "create_cylinder",
  "create_sphere",
  "create_cone",
  "boolean",
  "boolean_many",
  "revolve",
  "nurbs_loft",
  "shell",
  "fillet_edges",
  "chamfer_edges",
  "drill_pattern",
  "transform",
  "sketch_extrude",
  "psketch_extrude",
  "psketch_revolve",
  "import_step",
  "timeline_mould",
  "delete_part",
  "clear_parts",
]);

/**
 * How each base-taking mutating tool names the solid(s) it stacks work onto.
 * Only ops with a pre-existing base are listed — pure creators have nothing
 * to gate. boolean gates BOTH operands (an unsound tool solid poisons the
 * result exactly as an unsound base does); boolean_many gates the kept base
 * (its per-step certification already halts on a step that goes unsound).
 */
const BASE_REFS: Record<
  string,
  (args: any) => Array<{ uuid?: string; part_id?: number }>
> = {
  boolean: (a) => [{ uuid: a?.object_a }, { uuid: a?.object_b }],
  boolean_many: (a) => [{ uuid: a?.base }],
  shell: (a) => [{ uuid: a?.object }],
  drill_pattern: (a) => [{ uuid: a?.object }],
  transform: (a) => [{ uuid: a?.object }],
  fillet_edges: (a) => [{ part_id: a?.part_id }],
  chamfer_edges: (a) => [{ part_id: a?.part_id }],
  // Not a solid mutation, but the same inheritance argument: a sheet projected
  // from an unsound solid dimensions the defect as truth. acknowledge_unsound
  // is the deliberate inspection-sheet flow (a drawing OF the defect).
  make_drawing: (a) => [{ part_id: a?.part_id }],
  // Item 8 (audit S5, 2026-08-15): an STL/STEP/OBJ file on disk carries NO
  // ambient certificate — the exact argument gate 4 (sheet-export) was built
  // on, applied to the more common export path. `objects` empty means
  // "every solid" (io.ts: export_part) and is left UNCHECKED here — this
  // pre-flight can only gate refs it can name without a second fetch — the
  // server-side mirror in export.rs has no such gap: it iterates the
  // resolved solid set unconditionally, empty selection included.
  export_part: (a) =>
    Array.isArray(a?.objects)
      ? a.objects
          .filter((u: unknown) => typeof u === "string")
          .map((uuid: string) => ({ uuid }))
      : [],
};

/**
 * Gates whose underlying fact is LIVE state — kernel verdicts and sheet
 * certificates another author can change without a call from this session,
 * plus the single-point run counter, which this session's own NEXT call can
 * reset. Their refusals are never cached — every re-issue re-reads the live
 * fact, so repair (or a counter reset) unblocks the call immediately.
 */
const LIVE_FACT_GATES = new Set<string>([
  "unsound_base",
  "sheet_unsound",
  "sheet_quality",
  "sheet_uncertified",
  "single_point_run",
  // Item 9 (audit S11, 2026-08-15): the cumulative counter never resets on
  // its own next call the way the run counter does — it only ever grows —
  // so caching its refusal would not create the deadlock the doc comment
  // above warns about for gate 6. It is listed here for the SAME reason
  // `unsound_base` is: the underlying fact is live session state read fresh
  // on every dispatch (not a fixed fact about the arguments), so a cached
  // refusal could go STALE the moment the cumulative count changes shape
  // (e.g. a future change adds a way to lower it) — every re-issue re-reads
  // the counter rather than trusting a cached verdict about it.
  "single_point_cumulative",
  // Gate 6: the unverified-work list is live session state this session's own
  // next call clears. Caching it would deadlock precisely the caller who
  // COMPLIED — verify_part, then re-issue the identical checkpoint, and be
  // handed the stale refusal for a condition that no longer holds.
  "verification_scope",
]);

/**
 * The two verbs that count as having LOOKED at what was built (gate 6).
 *
 * `verify_part` reads the full certificate plus a diagnostic render of one
 * part; `verify_claim` checks a stated dimensional claim against the live
 * kernel. Both are read-only and neither can be satisfied by accident — an
 * agent that calls one has genuinely inspected its own output. `get_part` /
 * `list_parts` / `mass_properties` deliberately do NOT count: they report
 * facts without checking them against anything, which is exactly the
 * "certified sound, wrong shape" hole this gate exists to close.
 */
const VERIFIES = new Set<string>(["verify_part", "verify_claim"]);

/**
 * Checkpoint names that name a sequence position instead of a design intent
 * — exactly the names that would satisfy the intent gate while defeating its
 * purpose. Kept deliberately narrow (a generic word, an ordinal, or both):
 * "step 3", "op-2", "checkpoint", "7" are refused; any real intent phrase
 * ("bolt circle 8 x D18 on D160 B.C.", even the terse "cut cylinders")
 * passes — quality beyond this floor stays judgment, not schema.
 */
const GENERIC_CHECKPOINT_NAME =
  /^(?:(?:step|op|operation|cut|feature|part|checkpoint|chkpt|cp|test|wip|tmp|temp|misc)[\s\-_#:.]*)?\d*$/i;

/**
 * A clock/date reading dressed as a name — "Checkpoint 9:59:36 PM",
 * "10:05", "2026-08-01". The generic regex above PASSES these (its tail
 * only accepts a plain ordinal), and the frontend's old ◈ button minted
 * exactly this shape from the system clock, so this is a hole found in
 * the field, not a theoretical one. A time or a date carries less than
 * "step 3" — every timeline row already shows its timestamp. Mirrors
 * `CLOCK_CHECKPOINT_NAME` in `roshera-app/src/lib/timeline-events.ts`
 * and `api-server/src/handlers/timeline.rs`. The three copies cannot
 * share a constant across packages, but they are NOT hand-synced on
 * trust: `regex_copies_agree_across_the_three_packages` (in
 * timeline.rs's `checkpoint_name_gate_tests`) embeds this file's source
 * and fails `cargo test -p api-server` if this pattern's text drifts
 * from the other two.
 */
const CLOCK_CHECKPOINT_NAME =
  /^(?:(?:step|op|operation|checkpoint|chkpt|cp)[\s\-_#:.]*)?\d{1,4}([:\-/.]\d{1,2}){1,2}(\s*(am|pm))?$/i;

// ─── Session state (this process IS the session; a reconnect starts clean) ──

interface CachedRefusal {
  tool: string;
  turn: number;
  result: any;
}

/** Bounded FIFO refusal cache. 128 distinct refusals without an intervening
 *  state change is far beyond any real session; the bound exists so the map
 *  cannot grow without limit, not because eviction is expected. */
const REFUSAL_CACHE_MAX = 128;
const refusalCache = new Map<string, CachedRefusal>();

/** The currently open intent checkpoint (name + the turn it opened), or null.
 *  Opened by a successful timeline_checkpoint; a new checkpoint replaces it;
 *  clear_timeline (the ledger it lives in is wiped) closes it. */
let openIntent: { name: string; turn: number } | null = null;

/**
 * Solid-mutating tools that have SUCCEEDED under the currently open intent and
 * have NOT been looked at since (gate 6).
 *
 * Recorded on every successful `MUTATES_SOLIDS` dispatch while an intent is
 * open; emptied by a successful `verify_part` / `verify_claim`. The "since the
 * last mutation" scoping is what makes the record honest: a verify that ran
 * BEFORE the geometry it would have to check does not clear it, so the sequence
 * `checkpoint → verify_part → create_box → checkpoint` still meets the gate.
 * The box really was never inspected.
 *
 * Distinct verbs plus a count, not a list of every call: the set is bounded by
 * `MUTATES_SOLIDS` by construction — no growth limit needed, and therefore no
 * eviction that could make the reported count a lie — and "boolean, fillet_edges
 * across 40 calls" is the legible fact where forty repetitions of the word
 * "boolean" is not.
 */
let intentUnverified: { tools: Set<string>; count: number } = {
  tools: new Set(),
  count: 0,
};

/** A fresh, empty unverified-work record. */
function clearUnverified(): void {
  intentUnverified = { tools: new Set(), count: 0 };
}

/**
 * Read-only view of the open intent, for the HTTP client (`api()` in
 * core.ts) to stamp onto every backend call as `X-Roshera-Intent` /
 * `X-Roshera-Intent-Turn` — the wire link between the declaration the
 * intent gate already forced and the kernel ops it describes. The state
 * itself stays HERE (the gate opens/replaces/closes it); core.ts only
 * reads. Returns null when no intent is open — the headers are then
 * omitted entirely, so an absent intent stays absent on the wire.
 */
export function currentOpenIntent(): { name: string; turn: number } | null {
  return openIntent;
}

/** Hash key for the identical-call test: tool name + canonical (key-sorted,
 *  compact) JSON of the SDK-parsed args — defaults applied, so the direct
 *  path and the invoke path produce the same key for the same call. */
function refusalKey(tool: string, args: unknown): string {
  return fnv1a64hex(tool + "\0" + canonicalJson(args ?? {}));
}

// ─── Single-point-run counters (gate 5) ─────────────────────────────────────

/** Consecutive successful single-point additions allowed to one sketch before
 *  the next is refused. 8 keeps every legitimate hand-placed vertex set (a
 *  rectangle, a circle's centre+rim, a handful of constraint anchors) under
 *  the gate; anything longer is profile geometry, which is ONE polyline call. */
const SINGLE_POINT_RUN_MAX = 8;

/** Per-sketch run lengths of consecutive single-point additions. Cleared by
 *  ANY dispatch that is not itself a single-point addition (so counters only
 *  survive an unbroken run), and wholesale at 64 sketches as a hard bound
 *  against a pathological interleave — clearing early only ever ALLOWS calls. */
const pointRuns = new Map<string, number>();

/** Total single-point additions allowed to one sketch across the WHOLE
 *  session before every further one is refused, regardless of run resets
 *  (item 9, audit S11). 4x SINGLE_POINT_RUN_MAX: generous enough that
 *  legitimate named-anchor work spread across several short bursts (a
 *  handful of constraint points after each of a few separate profile
 *  edits) never trips it — the existing single_point_gate.test.mjs
 *  scenarios organically reach 24 cumulative points on one sketch across
 *  their run-reset exercises and none of them needed to change — but small
 *  enough that the interleave escape (8 points, 1 filler, repeat) is
 *  blocked at the 5th burst: 32 points, not the 256 the 1.3M-token failure
 *  was measured at. */
const SINGLE_POINT_CUMULATIVE_MAX = 32;

/** Per-sketch TOTAL of successful single-point additions, EVER — never
 *  reset by an intervening call (that is the entire point: the run counter
 *  alone is defeated by one filler call between every burst, S11). Not even
 *  a polyline call to the SAME sketch clears this: the round-trip/token
 *  cost of the single-point calls already made was already paid, and is
 *  not undone by later good behaviour — the cumulative total is a
 *  permanent fact about this sketch's history, not a debt later bulk work
 *  forgives. Cleared only by the same 64-sketch wholesale bound `pointRuns`
 *  uses (independently applied to this map's own size) and by
 *  resetSessionGates() (test seam). */
const pointCumulative = new Map<string, number>();

/**
 * The per-sketch counter key when `tool(args)` is a single-point addition,
 * else null. Two shapes qualify:
 *  - psketch_add_entity {kind:'point'} — the exact call the 256-point gear
 *    profile was measured burning 1.3M tokens on, one vertex per call;
 *  - sketch_points with a ONE-point array — the click-draft surface's version
 *    of the same loop (its description already says one backend mutation per
 *    point; a multi-point array is the legitimate use and never counts).
 */
function singlePointKey(tool: string, args: any): string | null {
  if (
    tool === "psketch_add_entity" &&
    args?.kind === "point" &&
    typeof args?.csketch_id === "string"
  ) {
    return `psketch_add_entity:${args.csketch_id}`;
  }
  if (
    tool === "sketch_points" &&
    Array.isArray(args?.points) &&
    args.points.length === 1 &&
    typeof args?.sketch_id === "string"
  ) {
    return `sketch_points:${args.sketch_id}`;
  }
  return null;
}

/** Test seam: drop all session gate state. Production never calls this — a
 *  fresh MCP process starts clean by construction. */
export function resetSessionGates(): void {
  refusalCache.clear();
  openIntent = null;
  clearUnverified();
  pointRuns.clear();
  pointCumulative.clear();
}

// ─── Live-verdict lookups (pre-flight, short budget, honest on failure) ─────

/** `e.message` when `e` is an Error, else its string form — the same idiom
 *  `core.ts`'s own error path uses, so a caught `ApiError` reports the exact
 *  `"<METHOD> <path> → timed out after Xms …"` / `"→ 404: …"` text it threw
 *  with, which is what lets a reader tell a timeout from a 404. */
function describeError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Resolve a public object UUID to its kernel part id via the scene snapshot
 * (the only agent-visible carrier of the UUID↔SolidId map).
 *
 * TWO outcomes are collapsed into `partId: null` by design, because they mean
 * the SAME thing to the caller — proceed, the op's own handler fails loudly
 * on a genuinely absent object: the snapshot fetch SUCCEEDED and the uuid is
 * simply not in the live scene. That is a completed pre-flight with a
 * definitive (if unhelpful) answer, not an unavailable one.
 *
 * A THIRD outcome is different in kind and gets its own arm: the fetch itself
 * failed (`err`, matching S4's `partIdForUuid` throws → null` citation). The
 * gate still proceeds — refusing on a transport hiccup would be the
 * approximation this gate's whole design avoids — but callers can now tell
 * "the ref checked out empty" from "the pre-flight never ran", which is
 * exactly the marker item 1 exists to attach.
 */
async function partIdForUuid(
  uuid: string,
): Promise<{ partId: number | null } | { unavailable: string }> {
  try {
    const snap = await api(
      "GET",
      "/api/scene/snapshot",
      undefined,
      PERCEPTION_TIMEOUT_MS,
    );
    const objects: any[] = Array.isArray(snap?.objects) ? snap.objects : [];
    for (const o of objects) {
      if (o?.id === uuid) {
        const sid = o?.analytical_geometry?.solid_id;
        return { partId: typeof sid === "number" ? sid : null };
      }
    }
    return { partId: null };
  } catch (err) {
    return { unavailable: describeError(err) };
  }
}

/**
 * The part's LIVE cheap verdict.
 *
 * `verdict: null` covers both "unreadable shape" (the response carried
 * neither `sound` nor `valid` as a boolean — never fabricated into one) and
 * is otherwise unchanged from before this item: that branch is not the S4
 * citation (`liveVerdict throws → null`, gates.ts:507-509 in the audit) and
 * is left exactly as it behaved previously — silent proceed, no marker.
 *
 * `unavailable` is the NEW arm: the fetch itself threw (timeout, network
 * error, non-2xx), which is the actual S4 fail-open path. The gate still
 * proceeds on this arm too — see module doc, gate 3 — but the caller can now
 * attach a reasoned marker instead of a silent skip.
 */
async function liveVerdict(
  partId: number,
): Promise<
  { verdict: { sound: boolean; verdict: string | null } | null } | { unavailable: string }
> {
  try {
    const p = await api(
      "GET",
      `/api/agent/parts/${partId}/perception`,
      undefined,
      PERCEPTION_TIMEOUT_MS,
    );
    const flag = p?.sound ?? p?.valid;
    if (typeof flag !== "boolean") return { verdict: null };
    return {
      verdict: {
        sound: flag,
        verdict: typeof p?.verdict === "string" ? p.verdict : null,
      },
    };
  } catch (err) {
    return { unavailable: describeError(err) };
  }
}

// ─── The gates ──────────────────────────────────────────────────────────────

function intentGateRefusal(tool: string) {
  return gateRefusal({
    gate: "intent",
    reason:
      `'${tool}' would mutate the model, but no design intent is open — the ` +
      "timeline would record a kernel operation with no engineering decision " +
      "attached, and the design history stops being a map of decisions.",
    how_to_proceed:
      "Open the feature first: timeline_checkpoint({ name: \"<the feature, its " +
      "governing dimensions, and where it sits — e.g. 'bolt circle 8 x D18 on " +
      "D160 B.C.'>\" }), then re-issue this call unchanged. One checkpoint " +
      "covers the feature it names; open a new one when the next feature " +
      "starts. The checkpoint writes the matching notebook line itself.",
  });
}

function genericNameRefusal(name: string) {
  return gateRefusal({
    gate: "intent",
    reason:
      `checkpoint name '${name}' names a sequence position, not a design ` +
      "intent — it would satisfy the letter of the intent gate while leaving " +
      "the timeline unreadable as a map of decisions.",
    how_to_proceed:
      "Name what a drawing would name: the feature, its governing dimensions, " +
      "and where it sits — e.g. 'bolt circle 8 x D18 on D160 B.C.' or " +
      "'M8 clearance holes, close fit, 4x base corners'. If you cannot yet " +
      "say what you are about to build in one such phrase, work that out " +
      "first.",
  });
}

function singlePointRunRefusal(tool: string, count: number) {
  return gateRefusal({
    gate: "single_point_run",
    reason:
      `${count} consecutive single-point additions to this sketch with no ` +
      `other call in between — '${tool}' costs one backend mutation and one ` +
      `round trip PER POINT, so laying a profile out point-by-point burns the ` +
      `rate budget and the context window for geometry one call already ` +
      `expresses (measured: a 256-point gear profile placed this way cost ` +
      `~1.3M tokens).`,
    points_placed: count,
    how_to_proceed:
      "Send the remaining vertices in ONE call: psketch_add_entity " +
      "{ csketch_id, kind:'polyline', params:{ points:[[x,y],…], closed:true } } " +
      "carries the entire loop — hundreds of vertices — in a single backend " +
      "mutation (on the click-draft surface, pass sketch_points the whole " +
      "points array at once, or better, move the profile to psketch_begin + " +
      "polyline). The points already placed are live and unaffected. " +
      "kind:'point' remains available for the few named vertices constraints " +
      "will reference; any other tool call resets this counter.",
  });
}

/**
 * Item 9 (audit S11). Refusal for the CUMULATIVE counter — a different
 * condition from the run counter above: this fires even when no unbroken
 * run ever reached SINGLE_POINT_RUN_MAX, because the caller has been
 * resetting the run counter with a filler call between bursts. Distinct
 * `gate` name (`single_point_cumulative`, not `single_point_run`) so a
 * trajectory can tell which condition actually tripped.
 */
function singlePointCumulativeRefusal(tool: string, count: number) {
  return gateRefusal({
    gate: "single_point_cumulative",
    reason:
      `${count} single-point additions to this sketch across the WHOLE ` +
      `session — not just the current unbroken run — regardless of how ` +
      `many other calls were interleaved between them. '${tool}' costs one ` +
      `backend mutation and one round trip PER POINT no matter what runs ` +
      `between them, so spacing a large point count out with a cheap ` +
      `filler call every few points still burns the same rate budget and ` +
      `context window the run counter alone exists to prevent (measured: ` +
      `a 256-point gear profile placed this way cost ~1.3M tokens, and one ` +
      `filler call per 8 points is enough to keep the run counter under ` +
      `its own limit forever).`,
    points_placed_cumulative: count,
    how_to_proceed:
      "Send the remaining vertices in ONE call: psketch_add_entity " +
      "{ csketch_id, kind:'polyline', params:{ points:[[x,y],…], closed:true } } " +
      "carries the entire loop in a single backend mutation. The points " +
      "already placed are live and unaffected. This cumulative total does " +
      "NOT reset — unlike the run counter, no intervening call (including " +
      "a polyline call to this SAME sketch) lowers it, because the round " +
      "trip already spent placing these points is not undone by later " +
      "bulk work.",
  });
}

function verificationScopeRefusal(
  closingIntent: string,
  distinct: string[],
  count: number,
  nextName: string,
) {
  return gateRefusal({
    gate: "verification_scope",
    reason:
      `the open intent '${closingIntent}' built geometry that was never ` +
      `checked: ${count} solid-mutating call(s) (${distinct.join(", ")}) ` +
      `ran under it and no verify_part / verify_claim followed the last of ` +
      `them. Opening '${nextName}' would close that intent with its result ` +
      `unexamined. A SOUND certificate is a statement about topology — closed, ` +
      `manifold, oriented — and says nothing about whether the geometry is the ` +
      `geometry you asked for: this kernel has shipped a certified-sound loft ` +
      `carrying a 9.97% shape error. Nothing downstream re-opens that question ` +
      `once the feature is closed.`,
    closing_intent: closingIntent,
    unverified_operations: distinct,
    unverified_count: count,
    how_to_proceed:
      "Look at what you built, then re-issue this call unchanged: " +
      "verify_part({ part_id }) returns the full certificate plus a diagnostic " +
      "render, and verify_claim({ ... }) checks a stated dimension against the " +
      "live kernel. Either one clears this gate for the work done so far. If " +
      "the previous feature genuinely does not need checking (scratch geometry, " +
      "a cutter you are about to subtract away), re-issue this exact call with " +
      "skip_verification: true — the intent then closes unverified ON THE " +
      "RECORD rather than by omission.",
  });
}

function unsoundBaseGateRefusal(
  tool: string,
  partId: number,
  verdict: string | null,
) {
  return gateRefusal({
    gate: "unsound_base",
    reason:
      `part ${partId} is UNSOUND by the kernel's live verdict` +
      (verdict ? ` (${verdict})` : "") +
      ` — '${tool}' would stack new work onto a defective solid, and every ` +
      "downstream certificate would inherit the defect.",
    unsound_base: { part_id: partId, verdict },
    how_to_proceed:
      "Diagnose with verify_part (full certificate + diagnostic render), then " +
      "repair or roll back before continuing. If THIS operation is itself the " +
      "deliberate repair (e.g. a boolean used to heal the shell, a rebuild " +
      "from a known-good state), re-issue this exact call with " +
      "acknowledge_unsound: true.",
  });
}

// ─── Sheet-export gate (gate 4) ─────────────────────────────────────────────

/**
 * The live sheet certificate for a drawing, or null when it cannot be read.
 * Uses the DEFAULT api timeout, not the short perception budget: the semantic
 * endpoint re-measures every fact against the live model, and a spuriously
 * short budget would refuse honest sheets on slow days — this gate fails
 * CLOSED, so its pre-flight read deserves the full budget.
 */
async function liveSheetCertificate(drawingId: string): Promise<any | null> {
  try {
    const r = await api("GET", `/api/drawings/${drawingId}/semantic`);
    const cert = r?.certificate;
    return cert && typeof cert === "object" ? cert : null;
  } catch {
    return null;
  }
}

/**
 * Refusal for drawing_export_sheet, or null to let the export proceed.
 * Checked in severity order: certificate unreadable, then stale/dangling
 * facts, then layout-quality Errors (the only one with a bypass).
 */
async function sheetExportGate(args: any): Promise<any | null> {
  const drawingId =
    typeof args?.drawing_id === "string" ? args.drawing_id : null;
  if (drawingId === null) return null; // schema validation rejects it loudly
  const cert = await liveSheetCertificate(drawingId);

  if (cert === null) {
    return gateRefusal({
      gate: "sheet_uncertified",
      reason:
        `the live certificate for drawing ${drawingId} could not be read, so ` +
        "nothing certifies its printed dimensions against the current model — " +
        "and a PDF/DXF/SVG on disk can never re-verify itself. Exporting an " +
        "uncertified sheet would ship an approximation labeled as exact.",
      how_to_proceed:
        "Confirm the drawing_id (make_drawing returned it) and that the " +
        "backend is reachable, read the certificate with " +
        "drawing_read_semantics, then re-issue this call. This refusal is " +
        "re-evaluated live on every attempt.",
    });
  }

  const counts = cert.counts ?? {};
  const stale = Number(counts.stale ?? 0);
  const dangling = Number(counts.dangling ?? 0);
  if (cert.sound === false || stale > 0 || dangling > 0) {
    const facts: any[] = Array.isArray(cert.facts) ? cert.facts : [];
    const offending = facts
      .filter(
        (f) => f?.live?.verdict === "stale" || f?.live?.verdict === "dangling",
      )
      .slice(0, 8)
      .map((f) => `${f.label} [${f.live.verdict}]`);
    return gateRefusal({
      gate: "sheet_unsound",
      reason:
        `drawing ${drawingId} is UNSOUND against the live model: ${stale} ` +
        `stale fact(s) (the model moved since this sheet was projected) and ` +
        `${dangling} dangling fact(s) (a referenced face no longer exists). ` +
        "A sheet whose printed dimensions disagree with the model would have " +
        "a shop machine the wrong part.",
      unsound_facts: offending,
      how_to_proceed:
        "Regenerate the sheet from the current model with make_drawing (it " +
        "returns a new drawing_id) and export that. There is no override: " +
        "regeneration is one cheap call, and no flow legitimately ships a " +
        "sheet that disagrees with the model it claims to describe.",
    });
  }

  const quality = cert.quality ?? null;
  if (quality?.passed === false && args?.acknowledge_layout_issues !== true) {
    const issues: any[] = Array.isArray(quality.issues) ? quality.issues : [];
    const errors = issues
      .filter((i) => String(i?.severity ?? "").toLowerCase() === "error")
      .slice(0, 8)
      .map((i) => (i?.view ? `${i.view}: ${i.message}` : i?.message));
    return gateRefusal({
      gate: "sheet_quality",
      reason:
        `drawing ${drawingId} failed its layout-quality certificate — ` +
        `${errors.length ? errors.length : issues.length} Error-severity ` +
        "finding(s) (label collisions, redundant dimensions, broken view " +
        "arrangement): exactly what a drawing checker rejects on sight.",
      layout_errors: errors,
      how_to_proceed:
        "Regenerate with make_drawing after fixing the cause where possible. " +
        "If a human asked to see the defective layout itself (a draft for " +
        "review, not a shop release), re-issue this exact call with " +
        "acknowledge_layout_issues: true.",
    });
  }

  return null;
}

/**
 * One base ref whose gate-3 pre-flight could NOT complete — a live fetch
 * (the snapshot resolve, or the perception read) threw rather than
 * answering. Distinct from a ref that resolved and turned out sound, and
 * distinct from a uuid that plainly is not in the live scene (that pre-flight
 * DID complete; the op's own handler fails loudly on it, which is not the
 * silent, byte-identical coin flip S4 is about).
 */
export interface GatePreflightGap {
  /** The uuid or `part <id>` this pre-flight step was about. */
  ref: string;
  /** Which pre-flight step failed to complete. */
  stage: "resolve" | "verify";
  /** The underlying error/timeout message, verbatim — this is what lets a
   *  reader tell a timeout from a 404 rather than a bare "unavailable". */
  reason: string;
}

/**
 * `preDispatchGate`'s return contract. Deliberately NOT `any | null`: a
 * refusal (`{refusal}`) always short-circuits, unchanged from before this
 * item. `{proceed: true}` lets the call through exactly as `null` used to —
 * `preflight`, when present, carries gate 3's fail-open gaps so the ToolTable
 * wrapper (registry.ts, the one call site) can attach them to whatever the
 * real handler returns.
 *
 * This is state carried in the RETURN VALUE, not in a module-level variable
 * read back by `recordDispatchOutcome` — gate 3's own skip paths span two
 * `await`s (`partIdForUuid`, `liveVerdict`), so if the SDK ever interleaves
 * two dispatches, a module-level "last preflight gap" would let dispatch B's
 * pre-flight clear dispatch A's gap before A's own outcome is recorded.
 * Threading the gap through the return value and then straight into the same
 * async call's own result is race-free by construction — there is nothing
 * for another interleaved dispatch to clobber.
 */
export type GateDecision =
  | { refusal: any }
  | { proceed: true; preflight?: GatePreflightGap[] };

/**
 * Pre-dispatch gate, called by the ToolTable wrapper before the real handler.
 * Returns `{refusal}` (the call never reaches the kernel) or `{proceed:
 * true, preflight?}` (the call proceeds, with zero or more fail-open notes
 * from gate 3). Order matters: the cache answers first (no kernel work at
 * all on an identical re-issue), then the local gates, then the one gate that
 * needs a live fetch.
 */
export async function preDispatchGate(
  tool: string,
  args: unknown,
  turn: number,
): Promise<GateDecision> {
  // 1. Identical re-issue of a refused call → the same refusal, from cache.
  const hit = refusalCache.get(refusalKey(tool, args));
  if (hit) {
    return {
      refusal: {
        ...hit.result,
        content: [
          ...hit.result.content,
          {
            type: "text" as const,
            text:
              `[refusal cache] this exact call was refused at turn ${hit.turn} ` +
              "and no state-changing operation has succeeded since — the answer " +
              "above is unchanged and was served without re-running anything. " +
              "An identical re-issue will keep receiving it: change the " +
              "arguments or the design, or escalate quoting the refusal.",
          },
        ],
      },
    };
  }

  // 1b. Single-point-run gate (gate 5): two live session-local counters,
  // never answered from the refusal cache — both are re-read on every
  // issue. The RUN counter resets on any other call (so the reset unblocks
  // the next point immediately); the CUMULATIVE counter (item 9, audit
  // S11) does not, which is what makes the interleave escape stop being
  // free — checked second, so a call that already trips the run counter
  // reports that reason, not the cumulative one.
  const spKey = singlePointKey(tool, args);
  if (spKey !== null) {
    const run = pointRuns.get(spKey) ?? 0;
    if (run >= SINGLE_POINT_RUN_MAX) {
      return { refusal: singlePointRunRefusal(tool, run) };
    }
    const cumulative = pointCumulative.get(spKey) ?? 0;
    if (cumulative >= SINGLE_POINT_CUMULATIVE_MAX) {
      return { refusal: singlePointCumulativeRefusal(tool, cumulative) };
    }
  }

  // 2. Intent gate: checkpoint quality, then checkpoint presence.
  if (tool === "timeline_checkpoint") {
    const name = typeof (args as any)?.name === "string" ? (args as any).name : "";
    const trimmed = name.trim();
    if (
      GENERIC_CHECKPOINT_NAME.test(trimmed) ||
      CLOCK_CHECKPOINT_NAME.test(trimmed)
    ) {
      return { refusal: genericNameRefusal(name) };
    }
    // 2b. Verification-scope gate (gate 6). A new checkpoint CLOSES the open
    // one — the only close this surface has — so this is the last moment the
    // previous feature's result can be questioned. Refused when work ran under
    // it unverified, unless the caller says so explicitly.
    if (
      openIntent !== null &&
      intentUnverified.count > 0 &&
      (args as any)?.skip_verification !== true
    ) {
      return {
        refusal: verificationScopeRefusal(
          openIntent.name,
          [...intentUnverified.tools],
          intentUnverified.count,
          trimmed,
        ),
      };
    }
    return { proceed: true }; // a real intent phrase — let the handler record it
  }
  if (MUTATES_SOLIDS.has(tool) && openIntent === null) {
    return { refusal: intentGateRefusal(tool) };
  }

  // 3. Unsound-base gate (live verdict; explicit acknowledgement bypasses).
  const extractRefs = BASE_REFS[tool];
  const preflight: GatePreflightGap[] = [];
  if (extractRefs && (args as any)?.acknowledge_unsound !== true) {
    for (const ref of extractRefs(args)) {
      let partId: number | null = null;
      if (typeof ref.part_id === "number") {
        partId = ref.part_id;
      } else if (typeof ref.uuid === "string" && ref.uuid.length > 0) {
        const resolved = await partIdForUuid(ref.uuid);
        if ("unavailable" in resolved) {
          preflight.push({
            ref: ref.uuid,
            stage: "resolve",
            reason: resolved.unavailable,
          });
          continue; // pre-flight could not complete → proceed; the handler
          // still runs and its own ambient certificate tells the truth
        }
        partId = resolved.partId;
      }
      if (partId === null) continue; // uuid genuinely not in the live scene —
      // the pre-flight DID complete; the handler fails loudly on its own
      const v = await liveVerdict(partId);
      if ("unavailable" in v) {
        preflight.push({
          ref: typeof ref.uuid === "string" && ref.uuid.length > 0 ? ref.uuid : `part ${partId}`,
          stage: "verify",
          reason: v.unavailable,
        });
        continue; // verdict unavailable → proceed; the op's own ambient
        // certificate still reports the truth (see module doc: no assertion
        // is made, so nothing is approximated).
      }
      if (v.verdict && v.verdict.sound === false) {
        return { refusal: unsoundBaseGateRefusal(tool, partId, v.verdict.verdict) };
      }
      // v.verdict === null → no live fact of a DIFFERENT kind (unreadable
      // response shape, not a fetch failure) → proceed silently, unchanged
      // from this gate's behaviour before this item.
    }
  }

  // 4. Sheet-export gate (live certificate; fails CLOSED — see module doc).
  if (tool === "drawing_export_sheet") {
    const sheetRefusal = await sheetExportGate(args);
    if (sheetRefusal) return { refusal: sheetRefusal };
  }

  return preflight.length > 0 ? { proceed: true, preflight } : { proceed: true };
}

/**
 * Merge gate 3's fail-open notes into a PROCEEDING op's own result — never a
 * side channel. `readToolResult` (roshera-rl/lib/mcp_session.mjs) builds its
 * `data` field from exactly the first text content block's JSON, and every
 * downstream reader (reward.mjs, episode.mjs) reads `data`, not
 * `structuredContent` — checked, not assumed, before choosing this over a
 * second content block or a `structuredContent` field, either of which a
 * trajectory would silently never read.
 *
 * Only merges when that block parses as a JSON OBJECT — the `ok()`/`okp()`
 * success shape. A `fail()` result is prose by construction (`ERROR: <msg>`)
 * and is left completely untouched: forcing a key into it would trade one
 * invisible failure mode for a worse one (corrupted tool output), and an
 * op's own failure already makes that step visibly different from a clean
 * pass — the ambiguity this item exists to remove is specifically between
 * two SUCCESSFUL, otherwise-identical results.
 *
 * A result that already carries `gate_preflight` (should never happen — no
 * handler sets this key) is left alone rather than overwritten, on the same
 * "never fabricate" discipline the rest of this module follows.
 *
 * NEVER THROWS. Any shape this cannot safely annotate is returned completely
 * unchanged — the disclosure itself must never become a new way for a call
 * to fail (an explicit constraint of this item).
 */
export function attachGatePreflightGaps(
  result: any,
  gaps: GatePreflightGap[] | undefined,
): any {
  if (!Array.isArray(gaps) || gaps.length === 0) return result;
  try {
    const content: any[] = Array.isArray(result?.content) ? result.content : [];
    const idx = content.findIndex(
      (c) => c?.type === "text" && typeof c.text === "string",
    );
    if (idx === -1) return result;
    const parsed = JSON.parse(content[idx].text);
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      return result;
    }
    if ("gate_preflight" in parsed) return result;
    const withMarker = {
      ...parsed,
      gate_preflight: "unavailable",
      gate_preflight_gaps: gaps,
    };
    const newContent = content.slice();
    newContent[idx] = { ...content[idx], text: JSON.stringify(withMarker, null, 2) };
    return { ...result, content: newContent };
  } catch {
    return result;
  }
}

/**
 * Post-dispatch bookkeeping, called by the ToolTable wrapper with whatever
 * the call produced (a gate refusal, a handler result, or a cache replay).
 *  - A typed refusal is cached for the identical-re-issue answer — except
 *    LIVE_FACT_GATES refusals (unsound bases, sheet certificates), whose
 *    underlying fact is live state (see module doc).
 *  - A non-refusal outcome from any state-changing tool drops the whole
 *    cache: the world may have changed, so every refusal must be re-earned.
 *  - A successful timeline_checkpoint opens the session's intent; a
 *    successful clear_timeline closes it (its ledger is gone).
 */
export function recordDispatchOutcome(
  tool: string,
  args: unknown,
  result: any,
  turn: number,
): void {
  // Gate 5 bookkeeping — runs for EVERY dispatch on every path (gated,
  // handled, or cache replay). A successful single-point addition extends its
  // sketch's run; ANY dispatch that is not a single-point addition resets all
  // runs (the "intervening call" that keeps legitimate small sketches
  // untouched). A refused or failed single-point call neither extends nor
  // resets — the run stands, so re-issuing a refused point keeps meeting the
  // gate instead of grinding it down.
  //
  // The CUMULATIVE counter (item 9) extends alongside the run counter on
  // every successful single-point addition, but is NEVER cleared by a
  // non-single-point dispatch — that asymmetry is the whole fix: a filler
  // call resets `pointRuns` (by design, so short legitimate sketches are
  // never bothered) but must NOT reset `pointCumulative`, or the interleave
  // escape S11 found would simply move here instead of closing.
  const spKey = singlePointKey(tool, args);
  if (spKey === null) {
    pointRuns.clear();
  } else if (result?.isError !== true) {
    if (pointRuns.size >= 64) pointRuns.clear();
    pointRuns.set(spKey, (pointRuns.get(spKey) ?? 0) + 1);
    if (pointCumulative.size >= 64) pointCumulative.clear();
    pointCumulative.set(spKey, (pointCumulative.get(spKey) ?? 0) + 1);
  }

  const refusal = typedRefusalOf(result);
  if (refusal) {
    if (refusal.gate !== undefined && LIVE_FACT_GATES.has(refusal.gate)) {
      return; // live fact — never cached
    }
    const key = refusalKey(tool, args);
    if (!refusalCache.has(key)) {
      if (refusalCache.size >= REFUSAL_CACHE_MAX) {
        const oldest = refusalCache.keys().next().value;
        if (oldest !== undefined) refusalCache.delete(oldest);
      }
      refusalCache.set(key, { tool, turn, result });
    }
    return;
  }
  if (!READ_ONLY.has(tool)) refusalCache.clear();
  if (result?.isError !== true) {
    if (tool === "timeline_checkpoint") {
      const name =
        typeof (args as any)?.name === "string" ? (args as any).name : "";
      openIntent = { name, turn };
      // A fresh intent starts with a clean record — whatever the previous one
      // carried was either verified or explicitly waived by `skip_verification`
      // at the gate above; either way it is settled and must not haunt the next
      // feature.
      clearUnverified();
    } else if (tool === "clear_timeline") {
      openIntent = null;
      clearUnverified();
    } else if (VERIFIES.has(tool)) {
      // The caller LOOKED. Everything built so far under this intent has been
      // examined; only mutations after this point can re-arm the gate.
      clearUnverified();
    } else if (MUTATES_SOLIDS.has(tool) && openIntent !== null) {
      intentUnverified.tools.add(tool);
      intentUnverified.count += 1;
    }
  }
}
