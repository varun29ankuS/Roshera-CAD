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
  handler: (args: any, extra?: any) => any;
}

/** True when `v` is a Zod schema (as opposed to a raw `{key: zodType}` shape). */
function isZodSchema(v: unknown): v is z.ZodTypeAny {
  return (
    !!v &&
    (typeof v === "object" || typeof v === "function") &&
    typeof (v as any).safeParse === "function"
  );
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
    this.add({ name, description: config.description ?? "", schema, handler });
  }

  private add(t: RegisteredTool): void {
    if (this.tools.has(t.name)) {
      // A duplicate name would silently shadow — refuse loudly at load time.
      throw new Error(`duplicate tool registration: ${t.name}`);
    }
    this.tools.set(t.name, t);
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

export type Bench =
  | "core"
  | "sketch"
  | "assembly"
  | "drawing"
  | "analysis"
  | "labels"
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
  "psketch_op",
  "assembly_verify",
  "assembly_mate",
  "assembly_solve",
  "assembly_certify",
  "assembly_dof",
  "assembly_drag",
  "assembly_interference",
  "drawing_read_semantics",
  "drawing_query",
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
  section_view: "core",
  verify_part: "core",
  mass_properties: "core",
  document_units: "core",
  set_part_color: "core",
  select_face: "core",
  select_edge: "core",
  import_step: "core",
  export_part: "core",
  get_pointer: "core",
  timeline_mould: "core",
  bind_parameter_name: "core",
  rebuild_certificate: "core",
  timeline_scrub: "core",
  clear_timeline: "core",
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
  // assembly
  assembly_verify: "assembly",
  assembly_create: "assembly",
  assembly_add_instance: "assembly",
  assembly_list_instances: "assembly",
  assembly_transform_instance: "assembly",
  assembly_view: "assembly",
  assembly_mate: "assembly",
  assembly_solve: "assembly",
  assembly_certify: "assembly",
  assembly_dof: "assembly",
  assembly_drag: "assembly",
  assembly_interference: "assembly",
  // drawing
  make_drawing: "drawing",
  drawing_read_semantics: "drawing",
  drawing_query: "drawing",
  drawing_export_sheet: "drawing",
  dimension_part: "drawing",
  gdt_datum: "drawing",
  gdt_fcf: "drawing",
  gdt_report: "drawing",
  // analysis
  point_query: "analysis",
  ray_query: "analysis",
  region_query: "analysis",
  occupancy_view: "analysis",
  part_coverage: "analysis",
  part_distance: "analysis",
  part_features: "analysis",
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
};

/** Bench + stability for a tool name. Meta-tools default to the `meta` bench. */
export function metaFor(name: string): ToolMeta {
  return {
    bench: BENCH_OF[name] ?? "meta",
    stability: EXPERIMENTAL.has(name) ? "experimental" : "stable",
  };
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
