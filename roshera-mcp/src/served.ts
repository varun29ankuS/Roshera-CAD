/**
 * Tools the BACKEND defines — the generic adapter (single-registry design,
 * stage 1; `roshera-eval/mcp/SINGLE_REGISTRY_DESIGN.md`).
 *
 * A served tool has exactly one definition: a JSON entry owned by api-server
 * (`roshera-backend/api-server/src/agent_tools/*.json`, compiled into the
 * server and listed by `GET /api/agent/tools`). This module registers every
 * served definition into the SAME tool table the hand-written tools use, so
 * the gates, `invoke`, `cad_program`, `workbench` and `find_tool` treat it
 * like any other tool — and there is no per-tool TypeScript for it at all:
 *
 *  - the zod schema the SDK validates with is CONVERTED from the served JSON
 *    Schema (`jsonSchemaToZod`), refusing any keyword it does not model;
 *  - the handler is one function that reads the definition's `call` (route,
 *    path placeholders, query, body template, result shape) and calls it;
 *  - bench / stability / effects come from the definition (served_meta.ts),
 *    and effects become MCP annotations.
 *
 * Where the definitions come from, in order:
 *  1. the live server's `GET /api/agent/tools` (what THIS backend serves);
 *  2. when that is unreachable, the backend's own checked-in definition files
 *     in this repository — the same bytes the server compiles in, not a copy
 *     kept here. The adapter says on stderr which one it used.
 * A listing whose registry MAJOR version differs from the one this adapter
 * speaks is refused whole — never half-registered.
 */

import { z } from "zod";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { ToolHost } from "./registry.js";
import { api, fail, BASE, AUTH_HEADERS } from "./core.js";
import { setServedMeta, type ServedEffects } from "./served_meta.js";

/** The registry format MAJOR version this adapter speaks. */
export const ADAPTER_REGISTRY_MAJOR = 2;

export type HttpMethod = "GET" | "POST" | "PATCH" | "DELETE";

export interface ServedRoute {
  method: HttpMethod;
  path: string;
  query?: string[];
  body?: unknown;
  requires?: string[];
  result?: Record<string, unknown>;
}

export interface ServedSwitch {
  switch: string;
  cases: Record<string, ServedRoute>;
}

export interface ServedDef {
  name: string;
  bench: string;
  stability: "stable" | "experimental";
  effects: ServedEffects;
  idempotent: boolean;
  summary: string;
  description: string;
  input_schema: Record<string, unknown>;
  call: ServedRoute | ServedSwitch;
  examples: { title: string; args: Record<string, unknown> }[];
}

export interface ServedRegistry {
  registry_version: string;
  tools: ServedDef[];
  /** Where the listing came from, for the startup log and describe_tool. */
  source: string;
}

export class ServedRegistryError extends Error {}

// ─── Loading ────────────────────────────────────────────────────────────────

/** Refuse a listing in a format this adapter does not speak. */
export function checkRegistryVersion(version: unknown, source: string): void {
  const major = typeof version === "string" ? Number(version.split(".")[0]) : NaN;
  if (major !== ADAPTER_REGISTRY_MAJOR) {
    throw new ServedRegistryError(
      `${source}: registry_version ${JSON.stringify(version)} is not major ${ADAPTER_REGISTRY_MAJOR}, ` +
        `the format this adapter speaks — refusing the whole listing rather than registering part of it`,
    );
  }
}

/** The backend's checked-in definition directory, found by walking up. */
export function definitionDir(): string | null {
  const override = process.env.ROSHERA_TOOL_DEFS_DIR?.trim();
  if (override) return existsSync(override) ? override : null;
  let dir = path.dirname(fileURLToPath(import.meta.url));
  for (let i = 0; i < 8; i++) {
    const candidate = path.join(dir, "roshera-backend", "api-server", "src", "agent_tools");
    if (existsSync(candidate)) return candidate;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

/** Read every `*.json` definition file the backend compiles in. */
export function loadCheckedInDefs(dir: string | null = definitionDir()): ServedRegistry {
  if (dir === null) {
    throw new ServedRegistryError(
      "the backend's tool definitions (roshera-backend/api-server/src/agent_tools) were not found " +
        "beside this package; set ROSHERA_TOOL_DEFS_DIR or run against a live backend",
    );
  }
  const files = readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .sort();
  if (files.length === 0) throw new ServedRegistryError(`${dir} holds no definition files`);
  const tools: ServedDef[] = [];
  let version: string | null = null;
  for (const f of files) {
    const full = path.join(dir, f);
    const parsed = JSON.parse(readFileSync(full, "utf8"));
    checkRegistryVersion(parsed?.registry_version, full);
    version = parsed.registry_version;
    if (!Array.isArray(parsed.tools)) throw new ServedRegistryError(`${full} has no tools array`);
    tools.push(...parsed.tools);
  }
  return { registry_version: version ?? "", tools, source: `checked-in definitions (${dir})` };
}

/** `GET /api/agent/tools` on the live backend; null when unreachable/absent. */
export async function fetchLiveDefs(timeoutMs = 3000): Promise<ServedRegistry | null> {
  let res: Response;
  try {
    res = await fetch(`${BASE}/api/agent/tools`, {
      headers: { "X-Roshera-Agent": "Claude", ...AUTH_HEADERS },
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch {
    return null;
  }
  if (!res.ok) return null;
  const body = await res.json();
  checkRegistryVersion(body?.registry_version, `${BASE}/api/agent/tools`);
  if (!Array.isArray(body?.tools)) {
    throw new ServedRegistryError(`${BASE}/api/agent/tools answered without a tools array`);
  }
  return { registry_version: body.registry_version, tools: body.tools, source: `live backend (${BASE})` };
}

// ─── JSON Schema → zod (the subset the definitions use; never guesses) ───────

const MODELLED = new Set([
  "type",
  "properties",
  "required",
  "additionalProperties",
  "items",
  "minItems",
  "maxItems",
  "minLength",
  "maxLength",
  "minimum",
  "maximum",
  "enum",
  "format",
  "pattern",
  "default",
  "description",
]);

/**
 * Convert a served JSON Schema into the zod schema the SDK validates calls
 * with. Any keyword outside the modelled subset is REFUSED with its path:
 * silently ignoring, say, a `oneOf` would accept calls the backend refuses.
 */
export function jsonSchemaToZod(
  s: unknown,
  at = "$",
  shared: Map<string, z.ZodTypeAny> = new Map(),
  describe = true,
): z.ZodTypeAny {
  if (s === null || typeof s !== "object" || Array.isArray(s)) {
    throw new ServedRegistryError(`${at}: a schema must be an object`);
  }
  const o = s as Record<string, any>;
  for (const k of Object.keys(o)) {
    if (!MODELLED.has(k)) {
      throw new ServedRegistryError(`${at}: JSON Schema keyword '${k}' is not modelled by the adapter`);
    }
  }
  let t: z.ZodTypeAny;
  if (Array.isArray(o.enum)) {
    if (o.enum.length === 0 || !o.enum.every((v: unknown) => typeof v === "string")) {
      throw new ServedRegistryError(`${at}: only non-empty string enums are modelled`);
    }
    t = z.enum(o.enum as [string, ...string[]]);
  } else {
    switch (o.type) {
      case undefined:
        // An untyped value (`{description}` only) — the backend's own type
        // decides; nothing here can validate it.
        t = z.any();
        break;
      case "string": {
        let x = z.string();
        if (typeof o.minLength === "number") x = x.min(o.minLength);
        if (typeof o.maxLength === "number") x = x.max(o.maxLength);
        if (o.format !== undefined) {
          if (o.format !== "uuid") throw new ServedRegistryError(`${at}: string format '${o.format}' is not modelled`);
          x = x.uuid();
        }
        if (typeof o.pattern === "string") x = x.regex(new RegExp(o.pattern));
        t = x;
        break;
      }
      case "number":
      case "integer": {
        let x = o.type === "integer" ? z.number().int() : z.number();
        if (typeof o.minimum === "number") x = x.min(o.minimum);
        if (typeof o.maximum === "number") x = x.max(o.maximum);
        t = x;
        break;
      }
      case "boolean":
        t = z.boolean();
        break;
      case "array": {
        let x = z.array(o.items === undefined ? z.any() : jsonSchemaToZod(o.items, `${at}.items`, shared));
        if (typeof o.minItems === "number") x = x.min(o.minItems);
        if (typeof o.maxItems === "number") x = x.max(o.maxItems);
        t = x;
        break;
      }
      case "object": {
        const props = (o.properties ?? {}) as Record<string, unknown>;
        const req = new Set<string>(Array.isArray(o.required) ? o.required : []);
        for (const r of req) {
          if (!(r in props)) throw new ServedRegistryError(`${at}: requires '${r}', which is not a property`);
        }
        const shape: Record<string, z.ZodTypeAny> = {};
        for (const [k, v] of Object.entries(props)) {
          // The parent attaches the description AFTER the optional wrapper,
          // so a shared object is referenced whole (see below).
          let field = jsonSchemaToZod(v, `${at}.${k}`, shared, false);
          if (!req.has(k) && !(field instanceof z.ZodDefault)) field = field.optional();
          const desc = (v as Record<string, unknown>).description;
          shape[k] = typeof desc === "string" ? field.describe(desc) : field;
        }
        // Two properties with the same object shape (assembly_mate's `a` and
        // `b`) share ONE zod object, so the listing the SDK emits refers to it
        // once (`$ref`) instead of paying for it twice — the same listing the
        // hand-written zod produced. Descriptions stay per property.
        const { description: _d, default: _f, ...shapeKey } = o;
        const key = JSON.stringify(shapeKey);
        const prior = shared.get(key);
        if (prior) {
          t = prior;
        } else {
          const obj = z.object(shape);
          t = o.additionalProperties === false ? obj.strict() : obj.passthrough();
          shared.set(key, t);
        }
        break;
      }
      default:
        throw new ServedRegistryError(`${at}: type ${JSON.stringify(o.type)} is not modelled`);
    }
  }
  if ("default" in o) t = t.default(o.default);
  if (describe && typeof o.description === "string") t = t.describe(o.description);
  return t;
}

// ─── Calling a definition ───────────────────────────────────────────────────

/** Row-major 4×4 from a translation + optional axis-angle rotation (degrees). */
export function poseMatrix(
  position?: number[],
  angle_deg?: number,
  axis?: number[],
): number[][] {
  let m = [
    [1, 0, 0, 0],
    [0, 1, 0, 0],
    [0, 0, 1, 0],
    [0, 0, 0, 1],
  ];
  if (angle_deg && axis) {
    const len = Math.hypot(axis[0], axis[1], axis[2]) || 1;
    const [x, y, zz] = [axis[0] / len, axis[1] / len, axis[2] / len];
    const a = (angle_deg * Math.PI) / 180;
    const c = Math.cos(a);
    const s = Math.sin(a);
    const t = 1 - c;
    m = [
      [t * x * x + c, t * x * y - s * zz, t * x * zz + s * y, 0],
      [t * x * y + s * zz, t * y * y + c, t * y * zz - s * x, 0],
      [t * x * zz - s * y, t * y * zz + s * x, t * zz * zz + c, 0],
      [0, 0, 0, 1],
    ];
  }
  if (position) {
    m[0][3] = position[0];
    m[1][3] = position[1];
    m[2][3] = position[2];
  }
  return m;
}

/** Sentinel: a body directive whose argument is absent and has no default. */
const OMIT = Symbol("omit");

/** Render a body template against the (validated) arguments. */
export function renderBody(tpl: unknown, args: Record<string, unknown>): unknown {
  if (Array.isArray(tpl)) return tpl.map((x) => renderBody(x, args)).filter((x) => x !== OMIT);
  if (tpl && typeof tpl === "object") {
    const o = tpl as Record<string, any>;
    if (typeof o.$arg === "string") {
      const v = args[o.$arg];
      if (v !== undefined) return v;
      return "default" in o ? o.default : OMIT;
    }
    if (o.$pose && typeof o.$pose === "object") {
      const p = o.$pose as Record<string, string>;
      const matrix = p.matrix ? args[p.matrix] : undefined;
      if (matrix !== undefined) return matrix;
      return poseMatrix(
        args[p.position] as number[] | undefined,
        args[p.angle_deg] as number | undefined,
        args[p.axis] as number[] | undefined,
      );
    }
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(o)) {
      const r = renderBody(v, args);
      if (r !== OMIT) out[k] = r;
    }
    return out;
  }
  return tpl;
}

/** Fill `{arg}` placeholders (URL-encoded) and append the query arguments. */
export function routeUrl(route: ServedRoute, args: Record<string, unknown>): string {
  const filled = route.path.replace(/\{([A-Za-z0-9_]+)\}/g, (_m, name: string) => {
    const v = args[name];
    if (v === undefined || v === null) {
      throw new ServedRegistryError(`path argument '${name}' is missing`);
    }
    return encodeURIComponent(String(v));
  });
  const q = new URLSearchParams();
  for (const name of route.query ?? []) {
    const v = args[name];
    if (v !== undefined && v !== null) q.set(name, String(v));
  }
  const qs = q.toString();
  return qs ? `${filled}?${qs}` : filled;
}

/** A typed client-side refusal: the call cannot be made as given. */
function argumentRefusal(tool: string, reason: string, details: Record<string, unknown>) {
  return {
    content: [{ type: "text" as const, text: `ERROR: ${tool}: ${reason}` }],
    isError: true as const,
    structuredContent: {
      reason: `${tool}: ${reason}`,
      error_code: "missing_parameter",
      retryable: false,
      details,
    },
  };
}

/** The route a call takes: the def's single route, or its switch case. */
export function pickRoute(
  def: ServedDef,
  args: Record<string, unknown>,
): { route: ServedRoute } | { refusal: ReturnType<typeof argumentRefusal> } {
  const call = def.call as any;
  let route: ServedRoute;
  let label = "";
  if (typeof call.switch === "string") {
    const chosen = args[call.switch];
    const r = typeof chosen === "string" ? call.cases?.[chosen] : undefined;
    if (!r) {
      return {
        refusal: argumentRefusal(def.name, `'${call.switch}' = ${JSON.stringify(chosen)} selects no route`, {
          parameter: call.switch,
          cases: Object.keys(call.cases ?? {}),
        }),
      };
    }
    route = r;
    label = ` ${call.switch} '${chosen}'`;
  } else {
    route = call as ServedRoute;
  }
  const missing = (route.requires ?? []).filter((n) => args[n] === undefined);
  if (missing.length > 0) {
    return {
      refusal: argumentRefusal(def.name, `${label.trim() || "this call"} needs ${missing.join(", ")}`, {
        missing,
      }),
    };
  }
  return { route };
}

function textResult(data: unknown) {
  const result: {
    content: { type: "text"; text: string }[];
    structuredContent?: Record<string, unknown>;
  } = { content: [{ type: "text" as const, text: JSON.stringify(data, null, 2) }] };
  if (data && typeof data === "object" && !Array.isArray(data)) {
    result.structuredContent = data as Record<string, unknown>;
  }
  return result;
}

/** Shape the route's answer as the definition's `result` says. */
export function shapeResult(route: ServedRoute, args: Record<string, unknown>, r: any) {
  const spec = route.result;
  if (!spec) return textResult(r);
  if (typeof spec.image === "string") {
    const fill = (tpl: string) =>
      tpl.replace(/\{(\$?)([A-Za-z0-9_]+)\}/g, (_m, fromResult: string, name: string) =>
        String(fromResult ? r?.[name] : args[name]),
      );
    return {
      content: [
        { type: "image" as const, data: r?.[spec.image], mimeType: "image/png" },
        { type: "text" as const, text: typeof spec.caption === "string" ? fill(spec.caption) : "" },
      ],
    };
  }
  const out: Record<string, unknown> = { ...(r && typeof r === "object" ? r : { value: r }) };
  for (const [from, to] of Object.entries((spec.rename ?? {}) as Record<string, string>)) {
    if (from in out) {
      out[to] = out[from];
      delete out[from];
    }
  }
  for (const name of (spec.echo ?? []) as string[]) out[name] = args[name];
  return textResult(out);
}

/** MCP tool annotations from a definition's declared effects. */
export function annotationsFor(def: ServedDef) {
  return {
    readOnlyHint: def.effects === "read_only",
    destructiveHint: def.effects === "destructive",
    idempotentHint: def.idempotent,
  };
}

/**
 * Register every served definition into `host`. Each definition is fully
 * converted BEFORE any is registered, so a definition the adapter cannot
 * model refuses the whole listing instead of leaving a partial bench.
 */
export function registerServedTools(host: ToolHost, reg: ServedRegistry): void {
  const converted = reg.tools.map((def) => {
    if (typeof def?.name !== "string" || def.name.length === 0) {
      throw new ServedRegistryError(`${reg.source}: a definition has no name`);
    }
    const schema = jsonSchemaToZod(def.input_schema, def.name);
    if (!(schema instanceof z.ZodObject)) {
      throw new ServedRegistryError(`${def.name}: input_schema must be an object schema`);
    }
    return { def, schema };
  });
  for (const { def, schema } of converted) {
    setServedMeta(def.name, {
      bench: def.bench,
      stability: def.stability,
      effects: def.effects,
      idempotent: def.idempotent,
    });
    host.registerTool(
      def.name,
      {
        description: def.description,
        inputSchema: schema,
        annotations: annotationsFor(def),
        served: def,
      },
      async (args: Record<string, unknown>) => {
        const picked = pickRoute(def, args);
        if ("refusal" in picked) return picked.refusal;
        try {
          const { route } = picked;
          const body = route.body === undefined ? undefined : renderBody(route.body, args);
          const answer = await api(route.method, routeUrl(route, args), body);
          return shapeResult(route, args, answer);
        } catch (e) {
          return fail(e);
        }
      },
    );
  }
}
