/**
 * The three meta-tools — the worst-case foundation (spec §Layer 2, §3).
 *
 * On ANY client, at fixed context cost (~3 tool definitions), the entire tool
 * inventory is reachable through a predictable funnel:
 *   - `find_tool`     — deterministic ranked search (no LLM, no embeddings)
 *   - `describe_tool` — full input schema + purpose on demand
 *   - `invoke`        — run any registry tool, validated by its OWN schema
 *
 * `invoke` dispatches to the SAME `{schema, handler}` the direct tool uses (from
 * the single ToolTable), and validates args through that schema BEFORE dispatch
 * with the SAME SDK helpers the direct call uses — so a meta-path call is never
 * less checked, and a validation failure is the identical typed error (§3.2).
 */

import { z } from "zod";
import {
  McpError,
  ErrorCode,
} from "@modelcontextprotocol/sdk/types.js";
import {
  normalizeObjectSchema,
  safeParseAsync,
  getParseErrorMessage,
} from "@modelcontextprotocol/sdk/server/zod-compat.js";
import {
  ToolTable,
  ToolHost,
  RegisteredTool,
  metaFor,
  toolJsonSchema,
  estimateTokens,
} from "./registry.js";
import { ok, fail } from "./core.js";

// ─── Registry drift warning (spec §3.4) ─────────────────────────────────────
//
// Set once at startup by consumeRegistry when the live backend registry hash
// disagrees with the MCP's compiled expectation. Surfaced ONCE per session in
// meta-tool output metadata, then cleared — loud but not nagging.

let pendingRegistryWarning: string | null = null;

export function setRegistryWarning(msg: string | null): void {
  pendingRegistryWarning = msg;
}

/** Return the drift warning once, then clear it (so it appears a single time). */
function takeRegistryWarning(): string | null {
  const w = pendingRegistryWarning;
  pendingRegistryWarning = null;
  return w;
}

/** Wrap a meta-tool payload, attaching the one-shot drift warning if pending. */
function okMeta(data: Record<string, unknown>) {
  const warning = takeRegistryWarning();
  return ok(warning ? { _registry_drift_warning: warning, ...data } : data);
}

// ─── Deterministic synonym table (project rule: no LLM / no embeddings) ──────
//
// Small curated equivalence groups. A query token expands to its group-mates,
// which then match tool names (strongly) or purposes (weakly). Bidirectional:
// every member maps to every other member of its group.

const SYNONYM_GROUPS: string[][] = [
  ["hole", "drill", "bore", "drilled", "counterbore"],
  ["cut", "difference", "subtract", "remove", "carve"],
  ["screenshot", "render", "view", "picture", "snapshot"],
  ["measure", "dimension", "distance", "gap", "clearance"],
  ["boolean", "union", "join", "merge", "combine", "fuse"],
  ["revolve", "lathe", "spin", "turn"],
  ["fillet", "round", "blend"],
  ["chamfer", "bevel"],
  ["sphere", "ball"],
  ["box", "cube", "block", "cuboid"],
  ["cylinder", "tube", "rod", "shaft"],
  ["cone", "frustum", "taper"],
  ["assembly", "assemble", "mate", "joint"],
  ["sketch", "draft", "profile"],
  ["mass", "weight", "volume", "inertia", "density"],
  ["section", "slice", "cutaway", "cross-section"],
  ["label", "name", "tag", "annotate"],
  ["export", "save", "write"],
  ["import", "load", "open"],
  ["shell", "hollow", "wall"],
];

const SYNONYMS: Map<string, Set<string>> = (() => {
  const m = new Map<string, Set<string>>();
  for (const group of SYNONYM_GROUPS) {
    for (const word of group) {
      const set = m.get(word) ?? new Set<string>();
      for (const other of group) if (other !== word) set.add(other);
      m.set(word, set);
    }
  }
  return m;
})();

const STOPWORDS = new Set([
  "a", "an", "the", "of", "for", "to", "in", "on", "with", "and", "or",
  "my", "me", "i", "this", "that", "it", "please", "how", "do", "can",
  "make", "get", "some", "at", "into", "from", "by", "as",
]);

/** Lowercase, split on non-alphanumerics, drop stopwords + empties. */
function tokenize(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((t) => t.length > 0 && !STOPWORDS.has(t));
}

// ─── Ranking (deterministic) ─────────────────────────────────────────────────

interface Scored {
  name: string;
  bench: string;
  purpose: string;
  token_estimate: number;
  score: number;
}

// Weights: an exact name match dominates; a query token (or its synonym) landing
// on a name token beats a landing in the purpose text. Name-token matches are
// IDF-scaled — a token unique to one tool (`render`, `drill`, `scene`) carries
// far more intent than a generic category suffix shared by many (`view`, `part`,
// `query`), so 'screenshot the scene' resolves to render_part/scene_view rather
// than every *_view tool. Small tie-breakers prefer the settled core.
const W_EXACT_NAME = 1000;
const W_NAME_TOKEN = 12; // × idf
const W_SYN_NAME = 8; //   × idf
const W_NAME_SUBSTR = 20;
const W_PURPOSE_WORD = 12;
const W_SYN_PURPOSE = 6;
const W_BENCH_CORE = 5;
const W_STABLE = 3;

/** Split a tool name into its lowercase tokens. */
function nameTokensOf(name: string): string[] {
  return name.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
}

/**
 * Inverse document frequency of each name-token across the table — deterministic
 * and computed once per search. A token in `df` of `N` tool names weighs
 * `ln((N+1)/df)`: unique tokens ≈ ln(N), ubiquitous suffixes ≈ small.
 */
function nameTokenIdf(table: ToolTable): { idf: Map<string, number>; n: number } {
  const df = new Map<string, number>();
  const all = table.all();
  for (const entry of all) {
    for (const tok of new Set(nameTokensOf(entry.name))) {
      df.set(tok, (df.get(tok) ?? 0) + 1);
    }
  }
  const n = all.length;
  const idf = new Map<string, number>();
  for (const [tok, d] of df) idf.set(tok, Math.log((n + 1) / d));
  return { idf, n };
}

function scoreTool(
  entry: RegisteredTool,
  queryTokens: string[],
  purposeLower: string,
  purposeWords: Set<string>,
  idf: Map<string, number>,
): number {
  const name = entry.name.toLowerCase();
  const nameTokens = new Set(nameTokensOf(name));
  const queryJoined = queryTokens.join("_");
  let score = 0;

  if (queryJoined === name) score += W_EXACT_NAME;

  const idfOf = (tok: string) => idf.get(tok) ?? Math.log(2);

  for (const qt of queryTokens) {
    if (nameTokens.has(qt)) score += W_NAME_TOKEN * idfOf(qt);
    else if (qt.length >= 3 && name.includes(qt)) score += W_NAME_SUBSTR;

    if (purposeWords.has(qt)) score += W_PURPOSE_WORD;
    else if (qt.length >= 4 && purposeLower.includes(qt)) score += W_PURPOSE_WORD;

    const syns = SYNONYMS.get(qt);
    if (syns) {
      for (const s of syns) {
        if (nameTokens.has(s)) score += W_SYN_NAME * idfOf(s);
        else if (purposeWords.has(s)) score += W_SYN_PURPOSE;
      }
    }
  }

  const { bench, stability } = metaFor(entry.name);
  if (score > 0) {
    if (bench === "core") score += W_BENCH_CORE;
    if (stability === "stable") score += W_STABLE;
  }
  return score;
}

/** Rank the whole table against a query; deterministic total order. */
export function rankTools(
  table: ToolTable,
  query: string,
  benchFilter?: string,
  limit = 5,
): Scored[] {
  const queryTokens = tokenize(query);
  const { idf } = nameTokenIdf(table);
  const scored: Scored[] = [];
  for (const entry of table.all()) {
    const { bench } = metaFor(entry.name);
    if (benchFilter && bench !== benchFilter) continue;
    const purposeLower = entry.description.toLowerCase();
    const purposeWords = new Set(tokenize(entry.description));
    const score = scoreTool(entry, queryTokens, purposeLower, purposeWords, idf);
    if (score <= 0) continue;
    scored.push({
      name: entry.name,
      bench,
      purpose: entry.description,
      token_estimate: estimateTokens(entry),
      score,
    });
  }
  // Deterministic order: score desc, then cheaper first, then name asc. Round
  // scores to avoid float dust reordering genuine ties.
  scored.sort(
    (a, b) =>
      Math.round((b.score - a.score) * 1e6) ||
      a.token_estimate - b.token_estimate ||
      a.name.localeCompare(b.name),
  );
  return scored.slice(0, Math.max(1, limit));
}

// ─── invoke validation (parity with a direct call) ──────────────────────────

/**
 * Validate `args` against a tool's own schema EXACTLY as the SDK's
 * `validateToolInput` does — same normalization, same parser, same error
 * message template. On failure throws the identical `McpError(InvalidParams,…)`
 * a direct call throws; the SDK's CallTool catch then renders both to the same
 * `{content:[{text}], isError:true}` result. Returns parsed data (defaults +
 * coercions applied) on success, so `invoke` dispatches the handler with the
 * same argument object a direct call would.
 */
export async function validateArgsLikeSdk(
  entry: RegisteredTool,
  args: unknown,
  toolName: string,
): Promise<any> {
  const inputObj = normalizeObjectSchema(entry.schema as any);
  const schemaToParse = inputObj ?? (entry.schema as any);
  const parseResult = await safeParseAsync(schemaToParse, args ?? {});
  if (!parseResult.success) {
    const error = "error" in parseResult ? parseResult.error : "Unknown error";
    const errorMessage = getParseErrorMessage(error);
    throw new McpError(
      ErrorCode.InvalidParams,
      `Input validation error: Invalid arguments for tool ${toolName}: ${errorMessage}`,
    );
  }
  return parseResult.data;
}

// ─── Registration ────────────────────────────────────────────────────────────

const FUNNEL_HINT =
  "The long tail lives behind three tools: find_tool (search) → describe_tool (schema) → invoke (run). " +
  "Every registered tool is reachable via invoke at fixed context cost, on any client.";

export function registerMetaTools(host: ToolHost, table: ToolTable): void {
  host.tool(
    "find_tool",
    "FUNNEL STEP 1/3 — deterministic ranked search over the FULL tool inventory " +
      "(all tools, not just the exposed surface). Give an intent in plain words; " +
      "get the top matches with name, bench, purpose, token cost. Then describe_tool " +
      "for the schema and invoke to run it. No LLM, no embeddings — name/synonym/" +
      "purpose ranking. " +
      FUNNEL_HINT,
    {
      query: z
        .string()
        .min(1)
        .describe("what you want to do, e.g. 'drill a bolt circle' or 'measure two faces'"),
      bench: z
        .enum(["core", "sketch", "assembly", "drawing", "analysis", "labels", "meta"])
        .optional()
        .describe("restrict results to one bench"),
      limit: z
        .number()
        .int()
        .min(1)
        .max(25)
        .optional()
        .describe("max results (default 5)"),
    },
    async ({ query, bench, limit }) => {
      const results = rankTools(table, query, bench, limit ?? 5);
      if (results.length === 0) {
        return okMeta({
          query,
          matches: [],
          note:
            "No tool matched. Broaden the query (fewer / more general words), drop the " +
            "bench filter, or try a synonym (e.g. 'cut' instead of 'subtract'). " +
            "Browse a whole bench by querying its name, e.g. 'analysis'.",
        });
      }
      return okMeta({
        query,
        matches: results.map((r) => ({
          name: r.name,
          bench: r.bench,
          purpose: r.purpose,
          token_estimate: r.token_estimate,
        })),
        next: "describe_tool({name}) for the full schema, then invoke({name, args}) to run it.",
      });
    },
  );

  host.tool(
    "describe_tool",
    "FUNNEL STEP 2/3 — the full input schema + purpose + bench + stability for one " +
      "tool, by exact name (from find_tool). This is how you learn a long-tail tool's " +
      "arguments without paying to keep its definition in context. Then invoke to run it. " +
      FUNNEL_HINT,
    {
      name: z.string().min(1).describe("exact tool name, e.g. 'drill_pattern'"),
    },
    async ({ name }) => {
      const entry = table.get(name);
      if (!entry) {
        const near = rankTools(table, name, undefined, 5).map((r) => r.name);
        return fail(
          new Error(
            `unknown tool '${name}'.` +
              (near.length ? ` Did you mean: ${near.join(", ")}?` : "") +
              " Use find_tool to search by intent.",
          ),
        );
      }
      const { bench, stability } = metaFor(name);
      return okMeta({
        name: entry.name,
        bench,
        stability,
        purpose: entry.description,
        token_estimate: estimateTokens(entry),
        input_schema: toolJsonSchema(entry),
        usage: `invoke({ name: '${entry.name}', args: { … } }) runs this tool; args are validated by this exact schema.`,
      });
    },
  );

  host.tool(
    "invoke",
    "FUNNEL STEP 3/3 — run ANY registered tool by name with its args, whether or " +
      "not it is in the exposed surface. Args are validated by the tool's OWN schema " +
      "first (identical typed error to a direct call on bad args), then dispatched to " +
      "the identical handler — a meta-path call is never less checked or less capable. " +
      FUNNEL_HINT,
    {
      name: z.string().min(1).describe("exact tool name (from find_tool / describe_tool)"),
      args: z
        .record(z.any())
        .optional()
        .describe("the tool's arguments object (validated by its own schema)"),
    },
    async ({ name, args }, extra) => {
      const entry = table.get(name);
      if (!entry) {
        const near = rankTools(table, name, undefined, 5).map((r) => r.name);
        return fail(
          new Error(
            `cannot invoke unknown tool '${name}'.` +
              (near.length ? ` Nearest matches: ${near.join(", ")}.` : "") +
              " Use find_tool to search by intent, then invoke the exact name.",
          ),
        );
      }
      // VALIDATION PARITY: parse through the tool's own schema exactly as a
      // direct call would; a bad arg throws the identical typed error here.
      const parsed = await validateArgsLikeSdk(entry, args ?? {}, name);
      // DISPATCH PARITY: the same handler the direct tool surface calls.
      return await entry.handler(parsed, extra);
    },
  );
}
