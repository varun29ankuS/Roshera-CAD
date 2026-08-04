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
// every member maps to every other member of its group. Members are STORED
// STEMMED (see `stem`), so one entry covers its whole inflection family —
// "print" covers printed/printing/printable.

const SYNONYM_GROUPS: string[][] = [
  ["hole", "drill", "bore", "counterbore"],
  ["cut", "difference", "subtract", "remove", "carve", "skim"],
  ["screenshot", "render", "view", "picture", "snapshot", "shot", "image"],
  ["measure", "dimension", "distance", "gap", "clearance", "size", "extents", "daylight", "envelope", "bounding", "bbox"],
  ["boolean", "union", "join", "merge", "combine", "fuse", "weld"],
  ["revolve", "lathe", "spin", "turn"],
  ["fillet", "round", "blend"],
  ["chamfer", "bevel", "deburr", "break"], // "break the (sharp) edges" — standard drawing-note language
  ["sphere", "ball"],
  ["box", "cube", "block", "cuboid"],
  ["cylinder", "tube", "rod", "shaft"],
  ["cone", "frustum", "taper"],
  ["assembly", "assemble", "mate", "joint", "hinge", "pivot", "revolute"],
  ["sketch", "draft", "profile"],
  ["mass", "weight", "heavy", "heaviness", "volume", "inertia", "density", "cg", "cog", "centroid", "gravity"],
  ["section", "slice", "cutaway"],
  ["label", "name", "tag", "annotate", "call"],
  ["export", "save", "write"],
  ["import", "load", "open"],
  ["shell", "hollow", "wall"],
  ["move", "translate", "shift", "reposition", "transform", "nudge", "rotate", "rotation", "reorient"],
  ["collide", "collision", "interference", "clash"],
  ["tolerance", "flatness", "perpendicularity", "gdt", "fcf", "datum", "callout"],
  ["perpendicular", "parallel", "constrain", "constraint", "coincident", "tangent"],
  ["dof", "freedom", "degrees", "underconstrained", "locked", "free"],
  ["machinable", "machined", "manufacturable", "cnc", "dfm", "manufacturability", "mill", "print", "printability"],
  // Engineering-shop vocabulary the table simply lacked (each word maps to the
  // canonical term a tool name/purpose actually uses):
  ["watertight", "sealed", "leak", "airtight"],
  ["color", "colour", "paint", "recolor", "tint"],
  ["ray", "raycast", "beam", "sightline"],
  ["undo", "revert", "rollback", "unwind"],
  ["redo", "reapply"],
  ["history", "log", "audit", "provenance", "who", "when"], // who/when = the history questions
  ["past", "ago", "earlier"],
  ["point", "coordinate", "probe"],
  ["scene", "everything", "entire", "whole"],
  ["list", "tree", "inventory", "enumerate"],
  ["claim", "arithmetic", "math", "formula", "equation"],
];

// ─── Conservative stemmer ────────────────────────────────────────────────────
//
// `tokenize` used to split only — so "dimensions" ≠ "dimension" and
// "filleting" ≠ "fillet", and every plural had to be hand-listed in the
// synonym table. This is a deliberately DUMB suffix stripper (plural / -ed /
// -ing / -ion / trailing-e), applied identically to query tokens, name tokens,
// purpose words, and the synonym table, so both sides land on the same stem:
// rotate/rotation → "rotat", crosses/crossings → "cross", move/moved → "mov".
// It is not Porter and does not try to be — identical treatment of both sides
// is what makes it safe.
function stem(t: string): string {
  let s = t;
  if (s.length >= 5 && s.endsWith("ies")) s = s.slice(0, -3) + "y";
  else if (s.length >= 4 && s.endsWith("es") && !s.endsWith("sses")) s = s.slice(0, -2);
  else if (s.length >= 4 && s.endsWith("s") && !s.endsWith("ss") && !s.endsWith("us")) s = s.slice(0, -1);
  if (s.length >= 6 && s.endsWith("ing")) s = s.slice(0, -3);
  else if (s.length >= 5 && s.endsWith("ed")) s = s.slice(0, -2);
  else if (s.length >= 6 && s.endsWith("ion")) s = s.slice(0, -3);
  if (s.length >= 4 && s.endsWith("e")) s = s.slice(0, -1);
  return s;
}

const SYNONYMS: Map<string, Set<string>> = (() => {
  const m = new Map<string, Set<string>>();
  for (const group of SYNONYM_GROUPS) {
    const stems = [...new Set(group.map(stem))];
    for (const word of stems) {
      const set = m.get(word) ?? new Set<string>();
      for (const other of stems) if (other !== word) set.add(other);
      m.set(word, set);
    }
  }
  return m;
})();

// ─── Multi-word phrases (normalised BEFORE tokenizing) ───────────────────────
//
// Single-token matching loses phrases whose meaning lives in the combination:
// "cross section" is a section, "roll back" is an undo, "line of sight" is a
// ray — none of which the individual words say ("line" alone must NOT pull in
// timeline tools). Each phrase rewrites to the canonical term before tokenize.
const PHRASES: Array<[RegExp, string]> = [
  [/\bcross[\s-]?sections?\b/g, " section "],
  [/\broll(?:ed|ing)?[\s-]?back\b/g, " undo "],
  [/\bline[\s-]of[\s-]sight\b/g, " ray "],
  [/\bcent(?:er|re)[\s-]of[\s-](?:gravity|mass)\b/g, " mass "],
  [/\bdegrees[\s-]of[\s-]freedom\b/g, " dof "],
  [/\blead[\s-]?ins?\b/g, " chamfer "],
  [/\bbill[\s-]of[\s-]materials\b/g, " bom "],
];

const STOPWORDS = new Set([
  "a", "an", "the", "of", "for", "to", "in", "on", "with", "and", "or",
  "my", "me", "i", "this", "that", "it", "please", "how", "do", "can",
  "make", "get", "some", "at", "into", "from", "by", "as", "check",
  // function words that were matching INSIDE unrelated names/purposes
  // ("over" ⊂ coverage/hover) or adding pure noise:
  "is", "are", "was", "were", "be", "been", "its", "whats", "what", "which",
  "we", "you", "your", "our", "us", "will", "would", "should", "yet", "did",
  "does", "just", "only", "so", "up", "out", "off", "over", "down", "end",
  "they", "them", "these", "those", "there", "here",
]);

/** Lowercase, rewrite known phrases, split, drop stopwords, stem. */
function tokenize(text: string): string[] {
  let t = text.toLowerCase();
  for (const [re, canon] of PHRASES) t = t.replace(re, canon);
  return t
    .split(/[^a-z0-9]+/)
    .filter((w) => w.length >= 2 && !STOPWORDS.has(w)) // 1-char tokens ("w", "x") are pure noise
    .map(stem);
}

// ─── Bounded fuzzy match (misspelling tolerance, still deterministic) ────────
//
// True iff `a` and `b` are within ONE edit (insert/delete/substitute). Used
// only for tokens ≥5 chars sharing a first letter, so "asembly" reaches
// "assembly" and "fillit" reaches "fillet" without short-word false positives.
function withinOneEdit(a: string, b: string): boolean {
  if (a === b) return true;
  const la = a.length;
  const lb = b.length;
  if (Math.abs(la - lb) > 1) return false;
  let i = 0;
  while (i < la && i < lb && a[i] === b[i]) i += 1;
  if (la === lb) return a.slice(i + 1) === b.slice(i + 1); // one substitution
  const [shorter, longer] = la < lb ? [a, b] : [b, a];
  return shorter.slice(i) === longer.slice(i + 1); // one insert/delete
}

// ─── Create-vs-mutate intent ─────────────────────────────────────────────────
//
// "name this face" wants label_create; "the label should say X" wants
// label_rename — vocabulary cannot separate them because both queries speak of
// labels and names. A verb implying FIRST-TIME creation boosts create-family
// tools; a verb implying change boosts mutate-family tools. Stems, matching
// `stem`'s output.
const CREATE_VERB_STEMS = new Set([
  "creat", "new", "start", "begin", "fresh", "setup", "spawn", "declar",
  "defin", "designat", "establish", "nam", "tag", "author", "initialis", "initializ",
]);
const MUTATE_VERB_STEMS = new Set(["renam", "chang", "edit", "updat", "modify", "adjust", "tweak", "correct"]);
const CREATE_FAMILY_NAME_STEMS = new Set(["creat", "add", "begin", "new"]);
const MUTATE_FAMILY_NAME_STEMS = new Set(["renam", "edit", "updat", "chang", "mould"]);

// ─── Ranking (deterministic) ─────────────────────────────────────────────────

interface Scored {
  name: string;
  bench: string;
  purpose: string;
  token_estimate: number;
  score: number;
}

// Weights: an exact name match dominates; a query token (or its synonym) landing
// on a name token beats a landing in the purpose text. Name-token AND purpose-
// word matches are IDF-scaled — a token unique to one tool (`render`, `drill`,
// `trim`) carries far more intent than one shared by half the registry (`part`,
// `face`, `view`), so a rare purpose word like "watertight" now genuinely pulls
// its tool up instead of drowning under generic name suffixes. Substring
// matching is PREFIX-anchored on tokens (interior substrings allowed only for
// long tokens, so "sketch" still reaches "psketch" while "line" can no longer
// match inside "timeline" and "name" inside "rename" — both measured failure
// modes). Small tie-breakers prefer the settled core.
const W_EXACT_NAME = 1000;
const W_NAME_TOKEN = 12; //  × name-token idf
const W_FUZZY_NAME = 9; //   × name-token idf (one-edit misspelling)
const W_SYN_NAME = 10; //    × name-token idf — a synonym landing on a name token
//                           is nearly as informative as the token itself
//                           ("collide" → interference), so it must not lose to
//                           an accumulation of generic tokens like "part"
const W_NAME_PREFIX = 10; // × name-token idf
const W_PURPOSE_WORD = 6; // × purpose-word idf
const W_SYN_PURPOSE = 3; //  × purpose-word idf
const W_INTENT = 14; //      create-vs-mutate verb agreement
const W_BENCH_CORE = 5;
const W_STABLE = 3;

/** Split a tool name into its lowercase STEMMED tokens. */
function nameTokensOf(name: string): string[] {
  return name.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean).map(stem);
}

/**
 * Inverse document frequency across the table — deterministic, computed once
 * per search, over BOTH name tokens and purpose words (stemmed). A token in
 * `df` of `N` docs weighs `ln((n+1)/df)`: unique ≈ ln(n), ubiquitous ≈ small.
 */
function buildIdf(table: ToolTable): { nameIdf: Map<string, number>; purposeIdf: Map<string, number> } {
  const nameDf = new Map<string, number>();
  const purposeDf = new Map<string, number>();
  const all = table.all();
  for (const entry of all) {
    for (const tok of new Set(nameTokensOf(entry.name))) nameDf.set(tok, (nameDf.get(tok) ?? 0) + 1);
    for (const w of new Set(tokenize(entry.description))) purposeDf.set(w, (purposeDf.get(w) ?? 0) + 1);
  }
  const n = all.length;
  const toIdf = (df: Map<string, number>) => {
    const idf = new Map<string, number>();
    for (const [tok, d] of df) idf.set(tok, Math.log((n + 1) / d));
    return idf;
  };
  return { nameIdf: toIdf(nameDf), purposeIdf: toIdf(purposeDf) };
}

function scoreTool(
  entry: RegisteredTool,
  queryTokens: string[],
  queryCreates: boolean,
  queryMutates: boolean,
  purposeWords: Set<string>,
  nameIdf: Map<string, number>,
  purposeIdf: Map<string, number>,
): number {
  const nameTokenList = nameTokensOf(entry.name);
  const nameTokens = new Set(nameTokenList);
  let score = 0;

  if (queryTokens.join("_") === nameTokenList.join("_")) score += W_EXACT_NAME;

  const idfOf = (tok: string) => nameIdf.get(tok) ?? Math.log(2);
  const pIdfOf = (w: string) => purposeIdf.get(w) ?? Math.log(2);

  for (const qt of queryTokens) {
    // name-token landing: exact stem > one-edit misspelling > prefix.
    if (nameTokens.has(qt)) score += W_NAME_TOKEN * idfOf(qt);
    else {
      let best = 0;
      for (const nt of nameTokenList) {
        if (qt.length >= 5 && qt[0] === nt[0] && withinOneEdit(qt, nt))
          best = Math.max(best, W_FUZZY_NAME * idfOf(nt));
        else if (qt.length >= 4 && (nt.startsWith(qt) || (qt.length >= 6 && nt.includes(qt))))
          best = Math.max(best, W_NAME_PREFIX * idfOf(nt));
      }
      score += best;
    }

    // purpose-word landing: exact stem, or prefix of a longer purpose word.
    if (purposeWords.has(qt)) score += W_PURPOSE_WORD * pIdfOf(qt);
    else if (qt.length >= 5) {
      let best = 0;
      for (const pw of purposeWords)
        if (pw.startsWith(qt)) best = Math.max(best, W_PURPOSE_WORD * pIdfOf(pw));
      score += best;
    }

    // synonym landing: the BEST group-mate match, not the sum — three group
    // members hitting one tool's purpose is one signal, not three.
    const syns = SYNONYMS.get(qt);
    if (syns) {
      let best = 0;
      for (const s of syns) {
        if (nameTokens.has(s)) best = Math.max(best, W_SYN_NAME * idfOf(s));
        else if (purposeWords.has(s)) best = Math.max(best, W_SYN_PURPOSE * pIdfOf(s));
      }
      score += best;
    }
  }

  // create-vs-mutate: verb intent agreeing with the tool's family.
  if (score > 0 && queryCreates !== queryMutates) {
    const family = queryCreates ? CREATE_FAMILY_NAME_STEMS : MUTATE_FAMILY_NAME_STEMS;
    if (nameTokenList.some((nt) => family.has(nt))) score += W_INTENT;
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
  const queryCreates = queryTokens.some((t) => CREATE_VERB_STEMS.has(t));
  const queryMutates = queryTokens.some((t) => MUTATE_VERB_STEMS.has(t));
  const { nameIdf, purposeIdf } = buildIdf(table);
  const scored: Scored[] = [];
  for (const entry of table.all()) {
    const { bench } = metaFor(entry.name);
    if (benchFilter && bench !== benchFilter) continue;
    const purposeWords = new Set(tokenize(entry.description));
    const score = scoreTool(entry, queryTokens, queryCreates, queryMutates, purposeWords, nameIdf, purposeIdf);
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

/**
 * Thrown by `validateOp` when a name is not in the table. Distinct from the
 * `McpError(InvalidParams)` a bad-ARGS validation throws, so callers can render
 * an unknown name (friendly "did you mean") differently from a schema failure
 * (the identical typed error a direct call throws), while both are still a
 * single up-front validation stop for `cad_program`.
 */
export class UnknownToolError extends Error {
  constructor(
    public readonly toolName: string,
    public readonly nearest: string[],
  ) {
    super(`unknown tool '${toolName}'`);
    this.name = "UnknownToolError";
  }
}

/**
 * Resolve + validate ONE op against the table exactly as `invoke` (and thus a
 * direct call) does: unknown name → `UnknownToolError` (with nearest matches),
 * bad args → the identical `McpError(InvalidParams,…)` a direct call throws.
 * Returns `{entry, parsed}` (defaults + coercions applied) on success. This is
 * the single shared resolve/validate path `invoke` and `cad_program` both run —
 * no second validator, no drift between meta-path and direct-path checking.
 */
export async function validateOp(
  table: ToolTable,
  name: string,
  args: unknown,
): Promise<{ entry: RegisteredTool; parsed: any }> {
  const entry = table.get(name);
  if (!entry) {
    const near = rankTools(table, name, undefined, 5).map((r) => r.name);
    throw new UnknownToolError(name, near);
  }
  const parsed = await validateArgsLikeSdk(entry, args ?? {}, name);
  return { entry, parsed };
}

// ─── Registration ────────────────────────────────────────────────────────────

export function registerMetaTools(host: ToolHost, table: ToolTable): void {
  host.tool(
    "find_tool",
    "FUNNEL STEP 1/3 — deterministic ranked search over the FULL tool inventory " +
      "(every registered tool, not just the exposed surface). Give an intent in " +
      "plain words; get top matches with name, bench, purpose, token cost. Then " +
      "describe_tool for the schema and invoke to run it — the whole long tail " +
      "is reachable that way at fixed context cost, on any client.",
    {
      query: z
        .string()
        .min(1)
        .describe("what you want to do, e.g. 'drill a bolt circle' or 'measure two faces'"),
      bench: z
        .enum(["core", "sketch", "assembly", "drawing", "analysis", "labels", "timeline", "meta"])
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
    "FUNNEL STEP 2/3 — full input schema + purpose + bench + stability for one " +
      "tool, by exact name (from find_tool). Learn a long-tail tool's arguments " +
      "before first call, without keeping its definition in context; then invoke runs it.",
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
      "the identical handler — never less checked or less capable than a direct call.",
    {
      name: z.string().min(1).describe("exact tool name (from find_tool / describe_tool)"),
      args: z
        .record(z.any())
        .optional()
        .describe("the tool's arguments object (validated by its own schema)"),
    },
    async ({ name, args }, extra) => {
      let resolved;
      try {
        // VALIDATION PARITY: the shared resolve/validate path — a bad arg throws
        // the identical McpError a direct call throws (bubbles to the SDK, same
        // typed result), which cad_program runs up front over its whole batch.
        resolved = await validateOp(table, name, args ?? {});
      } catch (e) {
        if (e instanceof UnknownToolError) {
          return fail(
            new Error(
              `cannot invoke unknown tool '${name}'.` +
                (e.nearest.length
                  ? ` Nearest matches: ${e.nearest.join(", ")}.`
                  : "") +
                " Use find_tool to search by intent, then invoke the exact name.",
            ),
          );
        }
        throw e; // bad-args McpError → identical typed error as a direct call
      }
      // DISPATCH PARITY: the same handler the direct tool surface calls.
      return await resolved.entry.handler(resolved.parsed, extra);
    },
  );
}
