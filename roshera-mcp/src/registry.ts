/**
 * The single internal tool table — Slice 2 / Layer 2 of the MCP scale
 * architecture (spec `2026-07-20-mcp-scale-architecture-design.md`).
 *
 * Every tool registers its `{name, description, zodSchema, handler}` triple into
 * ONE table here, exactly once. From that table BOTH surfaces are served:
 *   - the direct MCP tool surface (mounted onto the real McpServer in index.ts),
 *   - the meta-tool funnel (`find_tool` / `describe_tool` / `invoke`).
 * There is no second copy of a tool's schema or handler — no dual maintenance,
 * no drift between what a direct call does and what `invoke` does.
 *
 * `ToolHost` is the shim interface the `registerXxxTools(...)` modules write to.
 * `ToolTable` implements it by capturing instead of mounting; `McpServer` also
 * satisfies the shape, so a module could still target the real server directly,
 * but index.ts always routes through the table.
 */

import { z } from "zod";
import { toJsonSchemaCompat } from "@modelcontextprotocol/sdk/server/zod-json-schema-compat.js";
import { nextTurn } from "./core.js";
import { servedMeta } from "./served_meta.js";
import {
  preDispatchGate,
  recordDispatchOutcome,
  attachGatePreflightGaps,
  type GateDecision,
} from "./gates.js";

// ─── The capture shim ──────────────────────────────────────────────────────

/**
 * The subset of the McpServer registration surface the tool modules use. Two
 * forms exist in the codebase and both are captured:
 *   - `tool(name, description, rawZodShape, handler)`  (87 tools)
 *   - `registerTool(name, {description, inputSchema}, handler)`  (create_box/
 *     cylinder/sphere — their schema is a `z.object({...}).strict()`)
 */
export interface ToolHost {
  tool<S extends z.ZodRawShape>(
    name: string,
    description: string,
    shape: S,
    handler: (args: z.infer<z.ZodObject<S>>, extra?: any) => any,
  ): void;
  registerTool(
    name: string,
    config: { description?: string; inputSchema?: any; [k: string]: any },
    handler: (args: any, extra?: any) => any,
  ): void;
}

export interface RegisteredTool {
  name: string;
  description: string;
  /** Normalized Zod object schema — defaults, `.optional()`, `.strict()` all
   *  preserved verbatim, so `invoke` validates byte-for-byte as a direct call. */
  schema: z.ZodTypeAny;
  /** `decision` (3rd param, optional) is the SAME `GateDecision` the wrapper
   *  below just computed for THIS dispatch — never a separate, later read of
   *  gates.ts module state (M6, 2026-08-16 residuals). Every raw handler but
   *  `timeline_checkpoint`'s ignores it; it is threaded through unconditionally
   *  so the one handler that needs it never has to fall back to re-deriving
   *  the same fact itself. */
  handler: (args: any, extra?: any, decision?: GateDecision) => any;
  /** MCP tool annotations (read-only / destructive / idempotent hints). Set
   *  for backend-served tools from their declared effects (served.ts). */
  annotations?: Record<string, unknown>;
  /** The backend definition a served tool was registered from (served.ts);
   *  absent for a hand-written tool. */
  served?: unknown;
}

/** True when `v` is a Zod schema (as opposed to a raw `{key: zodType}` shape). */
function isZodSchema(v: unknown): v is z.ZodTypeAny {
  return (
    !!v &&
    (typeof v === "object" || typeof v === "function") &&
    typeof (v as any).safeParse === "function"
  );
}

// ─── The session's WORKING CONTEXT (find_tool reads it) ─────────────────────
//
// What the agent is doing right now, as a domain: the bench it entered, or the
// domain of its last WORKING call -- a sketch, assembly, drawing or simulate
// tool. A call that makes or changes a part (the create/modify tools) puts the
// context back on the part (null). Every other call -- a render, a list, a
// lookup, a question to the person, a checkpoint -- leaves it as it was: a read
// in the middle of a study does not end the study. find_tool uses it to give a
// word the meaning it has in that domain ("solve it" in a study is sim_static;
// "fix the bottom face" there is a support), so the same calls followed by the
// same query always give the same answer.

/** The benches that are a DOMAIN of work (a context), not a cross-cutting aid. */
export const DOMAIN_BENCHES: ReadonlySet<Bench> = new Set<Bench>(["sketch", "assembly", "drawing", "simulate"]);

let workingBench: Bench | null = null;
let workingFrom: string | null = null;
let shapeTools: ReadonlySet<string> = new Set();

/** The tools that make or change a part (set once by surface.ts from the create/modify modules). */
export function setShapeTools(names: Iterable<string>): void {
  shapeTools = new Set(names);
}

/** Record one dispatched call's effect on the working context. */
export function noteWorkingCall(name: string): void {
  const { bench } = metaFor(name);
  if (DOMAIN_BENCHES.has(bench)) {
    workingBench = bench;
    workingFrom = name;
  } else if (shapeTools.has(name)) {
    workingBench = null;
    workingFrom = name;
  }
}

/** Entering a bench sets the context to it; core_only clears it. */
export function noteBenchEntered(bench: Bench | null): void {
  workingBench = bench !== null && DOMAIN_BENCHES.has(bench) ? bench : null;
  workingFrom = bench === null ? null : `workbench('${bench}')`;
}

/** The current working context: the domain bench (or null = the part) and what set it. */
export function workingContext(): { bench: Bench | null; from: string | null } {
  return { bench: workingBench, from: workingFrom };
}

/** Back to no context (tests; a fresh session starts here). */
export function resetWorkingContext(): void {
  workingBench = null;
  workingFrom = null;
}

export class ToolTable implements ToolHost {
  private readonly tools = new Map<string, RegisteredTool>();

  tool(name: string, description: string, shape: any, handler: any): void {
    this.add({ name, description, schema: z.object(shape), handler });
  }

  registerTool(
    name: string,
    config: { description?: string; inputSchema?: any; [k: string]: any },
    handler: any,
  ): void {
    const raw = config.inputSchema;
    const schema: z.ZodTypeAny =
      raw === undefined
        ? z.object({})
        : isZodSchema(raw)
          ? raw
          : z.object(raw);
    this.add({
      name,
      description: config.description ?? "",
      schema,
      handler,
      ...(config.annotations ? { annotations: config.annotations } : {}),
      ...(config.served !== undefined ? { served: config.served } : {}),
    });
  }

  private add(t: RegisteredTool): void {
    if (this.tools.has(t.name)) {
      // A duplicate name would silently shadow — refuse loudly at load time.
      throw new Error(`duplicate tool registration: ${t.name}`);
    }
    // Tick the session turn counter (image-expiry policy, core.ts) exactly once
    // per handler dispatch, regardless of call path — direct mount (index.ts),
    // invoke() (metatools.ts), or cad_program()'s batch loop all call
    // `entry.handler(...)`, so wrapping it HERE is the single choke point that
    // covers all three without touching any of them.
    //
    // The same choke point runs the CONSTRAINT gates (gates.ts): the refusal
    // cache, the intent gate, and the unsound-base gate all fire before the
    // real handler, and every outcome (gated or handled) is recorded after —
    // so no call path can retry a refused call, build without a declared
    // intent, or stack work on an unsound base, without the gate seeing it.
    //
    // `preDispatchGate` returns a DECISION, not a nullable result: `{refusal}`
    // short-circuits exactly as before; `{proceed: true, preflight?}` lets the
    // real handler run and, when gate 3's live fetch could not complete for
    // one of the op's base refs, attaches that fact to the handler's OWN
    // result (`attachGatePreflightGaps`, gates.ts) before it is recorded or
    // returned — so a fail-open pre-flight stops being byte-identical to one
    // that actually ran (audit S4). The gap is carried in the return value of
    // THIS call, not in module state read back by `recordDispatchOutcome`:
    // gate 3's own skip paths span two `await`s, so a module-level "last gap"
    // would race under interleaved dispatches (see gates.ts's `GateDecision`
    // doc comment). The SAME decision object is also handed straight into
    // `rawHandler` as a 3rd argument (M6, 2026-08-16 residuals) — gate 6's
    // `gate6WouldHaveRefused` verdict rides along on it for exactly the same
    // reason: `timeline_checkpoint`'s handler must never take a second,
    // later read of `gates.ts`'s module state to reconstruct a fact this
    // call already decided.
    const rawHandler = t.handler;
    const wrapped: RegisteredTool["handler"] = async (args, extra) => {
      const turn = nextTurn();
      const decision = await preDispatchGate(t.name, args, turn);
      if ("refusal" in decision) {
        recordDispatchOutcome(t.name, args, decision.refusal, turn);
        return decision.refusal;
      }
      noteWorkingCall(t.name);
      let result = await rawHandler(args, extra, decision);
      if (decision.preflight && decision.preflight.length > 0) {
        result = attachGatePreflightGaps(result, decision.preflight);
      }
      recordDispatchOutcome(t.name, args, result, turn);
      return result;
    };
    this.tools.set(t.name, { ...t, handler: wrapped });
  }

  get(name: string): RegisteredTool | undefined {
    return this.tools.get(name);
  }
  has(name: string): boolean {
    return this.tools.has(name);
  }
  all(): RegisteredTool[] {
    return [...this.tools.values()];
  }
  names(): string[] {
    return [...this.tools.keys()];
  }
  get size(): number {
    return this.tools.size;
  }
}

// ─── Bench + stability classification (matches the kernel registry column) ──
//
// The kernel is the authority on this classification (agent_registry.rs). It is
// carried here as a compact name→{bench,stability} map — a few short strings per
// tool, NOT a second copy of any schema. When the backend registry is reachable
// and its hash verifies, its classification is preferred (see consumeRegistry).

// `timeline` (2026-07-31) covers history/branching/certification — a distinct
// concern from geometry creation, and arguably the most distinct one in this
// system, since it is the layer that makes everything else auditable. All 15
// tools live here on that criterion: the ten 2026-07-31 timeline verbs, the
// pre-existing timeline_mould/timeline_scrub, rebuild_certificate (a mould's
// certificate, not a shape), clear_timeline (resets a timeline branch), and
// bind_parameter_name (names a mould target — same #64 family as
// rebuild_certificate). None of the geometry-creation benches (core included)
// carry a history/branching/certification tool anymore.
export type Bench =
  | "core"
  | "sketch"
  | "assembly"
  | "drawing"
  | "analysis"
  | "simulate"
  | "labels"
  | "timeline"
  | "meta";
export type Stability = "stable" | "experimental";

interface ToolMeta {
  bench: Bench;
  stability: Stability;
}

const EXPERIMENTAL = new Set<string>([
  "nurbs_loft",
  "drill_pattern",
  "timeline_mould",
  "bind_parameter_name",
  "rebuild_certificate",
  "timeline_scrub",
  "timeline_history",
  "recipe_get",
  "timeline_branch",
  "timeline_branches",
  "timeline_switch",
  "timeline_merge",
  "timeline_conflicts",
  "timeline_checkpoint",
  "timeline_checkpoints",
  "timeline_undo",
  "timeline_redo",
  "psketch_op",
  "drawing_read_semantics",
  "drawing_query",
  "drawing_set_dimension",
  "drawing_set_projection",
  "drawing_set_title",
  "drawing_dimensions",
  "drawing_dimension_properties",
  "drawing_spaces",
  "drawing_look",
  "drawing_move_dimension",
  "drawing_align",
  "drawing_move_view",
  "drawing_reattach_leader",
  "drawing_remove_view",
  "drawing_remove_dimension",
  "iso286_limits",
  "dfm_check",
  "sim_mesh",
  "sim_setup",
  "sim_static",
  "sim_session",
  "sim_materials",
  "kb_lookup",
]);

const BENCH_OF: Record<string, Bench> = {
  // core
  workbench: "core",
  cad_program: "core",
  create_box: "core",
  create_cylinder: "core",
  create_sphere: "core",
  create_cone: "core",
  boolean: "core",
  boolean_many: "core",
  part_rename: "core",
  revolve: "core",
  nurbs_loft: "core",
  shell: "core",
  fillet_edges: "core",
  chamfer_edges: "core",
  transform: "core",
  drill_pattern: "core",
  delete_part: "core",
  clear_parts: "core",
  list_parts: "core",
  get_part: "core",
  render_part: "core",
  scene_view: "core",
  guide_attention: "core",
  section_view: "core",
  verify_part: "core",
  lens_explain: "core",
  lens_why_not_sound: "core",
  mass_properties: "core",
  document_units: "core",
  document_rename: "core",
  set_part_color: "core",
  select_face: "core",
  select_edge: "core",
  import_step: "core",
  export_part: "core",
  get_pointer: "core",
  user_context: "core",
  // timeline — history/branching/certification, a distinct concern from
  // geometry creation (see the Bench doc comment below).
  timeline_mould: "timeline",
  bind_parameter_name: "timeline",
  rebuild_certificate: "timeline",
  timeline_scrub: "timeline",
  clear_timeline: "timeline",
  timeline_history: "timeline",
  // Retrieval of a certified past build as a re-parameterizable plan. Same
  // history/certification concern as the rest of this bench — it reads the
  // recorded log and the certificates on it, and creates no geometry.
  recipe_get: "timeline",
  timeline_branch: "timeline",
  timeline_branches: "timeline",
  timeline_switch: "timeline",
  timeline_merge: "timeline",
  timeline_conflicts: "timeline",
  timeline_checkpoint: "timeline",
  timeline_checkpoints: "timeline",
  timeline_undo: "timeline",
  timeline_redo: "timeline",
  // sketch
  create_sketch: "sketch",
  sketch_add_shape: "sketch",
  sketch_points: "sketch",
  sketch_extrude: "sketch",
  plane_from_face: "sketch",
  psketch_begin: "sketch",
  psketch_add_entity: "sketch",
  psketch_constrain: "sketch",
  psketch_solve: "sketch",
  psketch_certify: "sketch",
  psketch_dof: "sketch",
  psketch_op: "sketch",
  psketch_extrude: "sketch",
  psketch_revolve: "sketch",
  // Sketch-on-face. Belongs on the sketch bench, not the meta fallback: an
  // agent that switches to `sketch` to build a profile is exactly the agent
  // that needs to anchor it to a face, and an unlisted tool is one it will not
  // reach for. Landing in the fallback is why the surface counts moved.
  psketch_plane_from_face: "sketch",
  // assembly: none here. The assembly tools are defined by the backend
  // (agent_tools/assembly.json) and classified from that definition
  // (served.ts / served_meta.ts), never from a copy in this table.
  // drawing
  make_drawing: "drawing",
  drawing_read_semantics: "drawing",
  drawing_query: "drawing",
  drawing_export_sheet: "drawing",
  drawing_set_dimension: "drawing",
  drawing_set_projection: "drawing",
  drawing_set_title: "drawing",
  drawing_dimensions: "drawing",
  // The space map: the agent sees a sheet as its scored spaces
  // (GET /api/drawings/{id}/spaces), graded by the worst.
  drawing_spaces: "drawing",
  // Agent eyes: the sheet or one space as an image (GET .../render.png),
  // the look shown in the person's app.
  drawing_look: "drawing",
  // Agent hands: one layout verb each (POST /api/drawings/{id}/layout),
  // certificate-gated, recorded and undoable, answered with the score change.
  drawing_move_dimension: "drawing",
  drawing_align: "drawing",
  drawing_move_view: "drawing",
  drawing_reattach_leader: "drawing",
  drawing_remove_view: "drawing",
  drawing_remove_dimension: "drawing",
  drawing_dimension_properties: "drawing",
  iso286_limits: "drawing",
  dimension_part: "drawing",
  gdt_datum: "drawing",
  gdt_fcf: "drawing",
  gdt_report: "drawing",
  // Correcting authored GD&T in place (each edit a recorded, undoable
  // `gdt_*` design event) instead of undoing the timeline past the mistake.
  gdt_fcf_update: "drawing",
  gdt_fcf_delete: "drawing",
  gdt_datum_update: "drawing",
  gdt_datum_delete: "drawing",
  gdt_dimensional_delete: "drawing",
  // analysis
  point_query: "analysis",
  ray_query: "analysis",
  region_query: "analysis",
  occupancy_view: "analysis",
  part_coverage: "analysis",
  part_distance: "analysis",
  part_features: "analysis",
  dfm_check: "analysis",
  // A linear static study (POST /api/agent/parts/{id}/sim/static): set-up
  // checks, mesh, solve, a verdict with its evidence. Reads the part, writes
  // nothing.
  sim_mesh: "simulate",
  sim_setup: "simulate",
  sim_static: "simulate",
  sim_session: "simulate",
  sim_materials: "simulate",
  kb_lookup: "analysis",
  ground_truth: "analysis",
  measure_faces: "analysis",
  verify_claim: "analysis",
  get_face: "analysis",
  get_revolve_profile: "analysis",
  // labels
  label_create: "labels",
  label_list: "labels",
  label_resolve: "labels",
  label_rename: "labels",
  label_delete: "labels",
  propose_labels: "labels",
  blackboard_add_entry: "labels",
  blackboard_edit_entry: "labels",
  blackboard_list: "labels",
  blackboard_clear: "labels",
  // The closed-question card builder lives with the notebook verbs: its
  // output is a blackboard line. Full table + labels bench, discovery via
  // find_tool (the kb_lookup precedent) — asking the human a closed
  // question is per-decision, not per-feature, so it does not earn
  // minimal-surface residency.
  ask_choice: "labels",
};

/** Bench + stability for a tool name. Meta-tools default to the `meta` bench. */
export function metaFor(name: string): ToolMeta {
  // A backend-served tool is classified by its own definition.
  const served = servedMeta(name);
  if (served) return { bench: served.bench as Bench, stability: served.stability };
  return {
    bench: BENCH_OF[name] ?? "meta",
    stability: EXPERIMENTAL.has(name) ? "experimental" : "stable",
  };
}

/**
 * Read-only view of the RAW explicit-classification table — every name that
 * has ever been given a bench, unfiltered by whether it is still registered.
 * Deliberately NOT `metaFor`: `metaFor`'s `?? "meta"` fallback is exactly what
 * makes a stale entry invisible (an unregistered name simply never gets
 * looked up through it), so a staleness gate must read this map directly, not
 * go through the fallback. Returns a frozen shallow copy — callers cannot
 * mutate the module's private table.
 */
export function explicitBenchTable(): Readonly<Record<string, Bench>> {
  return Object.freeze({ ...BENCH_OF });
}

// ─── Schema → JSON-schema + token estimate ──────────────────────────────────

/**
 * The JSON schema the MCP actually advertises for a tool — generated by the
 * SAME `toJsonSchemaCompat` the SDK uses to build `tools/list`, so `describe_tool`
 * and the token bill reflect exactly what a client pays for.
 */
export function toolJsonSchema(entry: RegisteredTool): Record<string, unknown> {
  try {
    return toJsonSchemaCompat(entry.schema as any, {
      strictUnions: true,
      pipeStrategy: "input",
    });
  } catch {
    // Never let a schema quirk crash description; degrade to an object stub.
    return { type: "object" };
  }
}

/**
 * Deterministic token-cost proxy for one tool definition: `ceil(len/4)` over the
 * compact JSON of `{name, purpose, input_schema}` — the same ≈4-chars/token rule
 * the kernel registry's `token_estimate` uses, applied to the MCP's own
 * advertised surface. Not a tokenizer; a stable budgeting signal.
 */
export function estimateTokens(entry: RegisteredTool): number {
  const payload = JSON.stringify({
    name: entry.name,
    purpose: entry.description,
    input_schema: toolJsonSchema(entry),
  });
  return Math.ceil(payload.length / 4);
}

// ─── Canonical JSON + FNV-1a-64 (registry drift hash) ───────────────────────

/**
 * Canonical JSON: keys sorted lexicographically at every level, compact (no
 * whitespace) — byte-compatible with serde_json's `to_string` over a BTreeMap
 * (the form the kernel hashes in agent_registry.rs). The two sides hash the
 * identical bytes, so a mismatch is real drift, never an encoder skew.
 */
export function canonicalJson(v: unknown): string {
  if (v === null || typeof v === "number" || typeof v === "boolean") {
    return JSON.stringify(v);
  }
  if (typeof v === "string") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map(canonicalJson).join(",") + "]";
  if (typeof v === "object") {
    const keys = Object.keys(v as Record<string, unknown>).sort();
    return (
      "{" +
      keys
        .map((k) => JSON.stringify(k) + ":" + canonicalJson((v as any)[k]))
        .join(",") +
      "}"
    );
  }
  return JSON.stringify(v ?? null);
}

/**
 * FNV-1a 64-bit → 16 lowercase hex chars. The identical algorithm and constants
 * (offset basis 0xcbf29ce484222325, prime 0x100000001b3) the kernel's `fnv1a_64`
 * uses, so a hash computed here over the same canonical bytes equals the kernel's
 * `registry_hash`. Implemented with BigInt for exact 64-bit wraparound.
 */
export function fnv1a64hex(input: string | Uint8Array): string {
  const bytes =
    typeof input === "string" ? new TextEncoder().encode(input) : input;
  const MASK = (1n << 64n) - 1n;
  const PRIME = 0x100000001b3n;
  let hash = 0xcbf29ce484222325n;
  for (const b of bytes) {
    hash ^= BigInt(b);
    hash = (hash * PRIME) & MASK;
  }
  return hash.toString(16).padStart(16, "0");
}
