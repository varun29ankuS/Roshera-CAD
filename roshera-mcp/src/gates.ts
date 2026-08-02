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
 * Solid-mutating verbs the intent gate covers: everything that creates or
 * reshapes a solid — the calls the policy's "checkpoint before the first
 * mutating call of every feature" was written about. Sketch construction
 * (psketch_begin/add_entity/constrain/solve) is deliberately NOT gated: the
 * feature materialises at the extrude/revolve, and nagging during sketch
 * iteration would teach the model to open garbage checkpoints. Assembly and
 * label tools are out of scope here (assembly placement policy is mid-flight
 * in the audit-fix wave, audit §6).
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
};

/**
 * Gates whose underlying fact is LIVE state (kernel verdicts, sheet
 * certificates) that another author can change without a call from this
 * session. Their refusals are never cached — every re-issue re-reads the
 * live fact, so repair by anyone unblocks the call immediately.
 */
const LIVE_FACT_GATES = new Set<string>([
  "unsound_base",
  "sheet_unsound",
  "sheet_quality",
  "sheet_uncertified",
]);

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

/** Hash key for the identical-call test: tool name + canonical (key-sorted,
 *  compact) JSON of the SDK-parsed args — defaults applied, so the direct
 *  path and the invoke path produce the same key for the same call. */
function refusalKey(tool: string, args: unknown): string {
  return fnv1a64hex(tool + "\0" + canonicalJson(args ?? {}));
}

/** Test seam: drop all session gate state. Production never calls this — a
 *  fresh MCP process starts clean by construction. */
export function resetSessionGates(): void {
  refusalCache.clear();
  openIntent = null;
}

// ─── Live-verdict lookups (pre-flight, short budget, honest on failure) ─────

/** Resolve a public object UUID to its kernel part id via the scene snapshot
 *  (the only agent-visible carrier of the UUID↔SolidId map). null = not
 *  resolvable — the op proceeds and fails loudly in its own handler. */
async function partIdForUuid(uuid: string): Promise<number | null> {
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
        return typeof sid === "number" ? sid : null;
      }
    }
    return null;
  } catch {
    return null;
  }
}

/** The part's LIVE cheap verdict. null = unavailable (never fabricated). */
async function liveVerdict(
  partId: number,
): Promise<{ sound: boolean; verdict: string | null } | null> {
  try {
    const p = await api(
      "GET",
      `/api/agent/parts/${partId}/perception`,
      undefined,
      PERCEPTION_TIMEOUT_MS,
    );
    const flag = p?.sound ?? p?.valid;
    if (typeof flag !== "boolean") return null;
    return {
      sound: flag,
      verdict: typeof p?.verdict === "string" ? p.verdict : null,
    };
  } catch {
    return null;
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
 * Pre-dispatch gate, called by the ToolTable wrapper before the real handler.
 * Returns a refusal result (the call never reaches the kernel) or null (the
 * call proceeds). Order matters: the cache answers first (no kernel work at
 * all on an identical re-issue), then the local gates, then the one gate that
 * needs a live fetch.
 */
export async function preDispatchGate(
  tool: string,
  args: unknown,
  turn: number,
): Promise<any | null> {
  // 1. Identical re-issue of a refused call → the same refusal, from cache.
  const hit = refusalCache.get(refusalKey(tool, args));
  if (hit) {
    return {
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
    };
  }

  // 2. Intent gate: checkpoint quality, then checkpoint presence.
  if (tool === "timeline_checkpoint") {
    const name = typeof (args as any)?.name === "string" ? (args as any).name : "";
    const trimmed = name.trim();
    if (
      GENERIC_CHECKPOINT_NAME.test(trimmed) ||
      CLOCK_CHECKPOINT_NAME.test(trimmed)
    ) {
      return genericNameRefusal(name);
    }
    return null; // a real intent phrase — let the handler record it
  }
  if (MUTATES_SOLIDS.has(tool) && openIntent === null) {
    return intentGateRefusal(tool);
  }

  // 3. Unsound-base gate (live verdict; explicit acknowledgement bypasses).
  const extractRefs = BASE_REFS[tool];
  if (extractRefs && (args as any)?.acknowledge_unsound !== true) {
    for (const ref of extractRefs(args)) {
      let partId: number | null = null;
      if (typeof ref.part_id === "number") partId = ref.part_id;
      else if (typeof ref.uuid === "string" && ref.uuid.length > 0) {
        partId = await partIdForUuid(ref.uuid);
      }
      if (partId === null) continue; // unresolvable → the handler fails loudly itself
      const v = await liveVerdict(partId);
      if (v && v.sound === false) {
        return unsoundBaseGateRefusal(tool, partId, v.verdict);
      }
      // v === null → verdict unavailable → proceed; the op's own ambient
      // certificate still reports the truth (see module doc: no assertion is
      // made, so nothing is approximated).
    }
  }

  // 4. Sheet-export gate (live certificate; fails CLOSED — see module doc).
  if (tool === "drawing_export_sheet") {
    return sheetExportGate(args);
  }

  return null;
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
    } else if (tool === "clear_timeline") {
      openIntent = null;
    }
  }
}
