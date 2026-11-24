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
  workingContext,
  Bench,
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
  // "load" is NOT here: in a CAD tool a load is a force ("strong enough under
  // load" ranked import_step first). "load the step file" still finds
  // import_step through "step" and "file".
  ["import", "open"],
  // A strength study's set-up words (2026-11-24): what pushes on the part...
  ["load", "force", "push", "pull", "press", "newton"],
  // ...and what holds it.
  ["support", "fixture", "restrain", "restraint", "held", "hold", "anchor"],
  // Material names are NOT a global group: "how heavy is this part in
  // aluminium" is a mass question. In a study they mean the material library
  // (DOMAIN_WORDS.simulate).
  ["shell", "hollow", "wall"],
  ["move", "translate", "shift", "reposition", "transform", "nudge", "rotate", "rotation", "reorient"],
  ["collide", "collision", "interference", "clash"],
  ["tolerance", "flatness", "perpendicularity", "gdt", "fcf", "datum", "callout"],
  ["perpendicular", "parallel", "constrain", "constraint", "coincident", "tangent"],
  ["dof", "freedom", "degrees", "underconstrained", "locked", "free"],
  ["machinable", "machined", "manufacturable", "cnc", "dfm", "manufacturability", "mill", "print", "printability"],
  ["strength", "strong", "stress", "fea", "simulate", "simulation", "deflection", "stiffness", "stiff", "safety"],
  ["mesh", "meshing", "elements", "nodes", "tets"],
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
  // A question about what is built from what is a question about the feature
  // graph (timeline_dependencies): "built on top of", "what else rebuilds",
  // "downstream of", "depends on". Phrases, not single words: "affect" alone
  // is a study word ("will the load affect it").
  [/\bbuilt\s+(?:on\s+top\s+of|on|from|off)\b/g, " dependencies downstream "],
  [/\bwhat\s+else\s+(?:rebuilds?|changes?|updates?|breaks?|moves?)\b/g, " dependencies downstream "],
  [/\b(?:down|up)stream(?:\s+of)?\b/g, " dependencies downstream "],
  [/\bdepends?\s+on\b|\bdependen(?:t|ts|cy|cies)\b/g, " dependencies downstream "],
  // "step 6" is a step of the history, never the STEP file format.
  [/\bstep\s+\d+\b/g, " event "],
  // A number followed by N / kN is a force ("push with 200 N"): the unit
  // alone is a 1-character token tokenize drops. Read as "force", never
  // "newton": psketch_solve's purpose speaks of Newton's METHOD.
  [/\b\d+(?:\.\d+)?\s?k?n\b/g, " force "],
  [/\bnewtons?\b/g, " force "],
  [/\bdegrees[\s-]of[\s-]freedom\b/g, " dof "],
  [/\blead[\s-]?ins?\b/g, " chamfer "],
  [/\bbill[\s-]of[\s-]materials\b/g, " bom "],
  // A kind of drawing is a request for a NEW sheet: "make a technical drawing
  // of the part" ranked make_drawing third -- "make" is a stopword and
  // "drawing" is shared by every drawing_* tool, while "part" hit part_rename's
  // name. Only make_drawing's purpose says "blueprint" (2026-11-19).
  [/\b(?:technical|engineering|manufacturing|production|shop|workshop|detail|2d)[\s-]drawings?\b/g, " blueprint drawing "],
  // A standard's number IS its identity: "ISO 2768" / "EN 1092-1" / "DIN 912"
  // must survive the bare-number drop in `tokenize`, so the prefix is glued
  // to its number ("iso2768") — a mixed token that is kept and matches the
  // same glued form in a description.
  [/\b(iso|din|en|astm|ansi|asme|bs|jis)[\s-]?(\d+)\b/g, " $1$2 "],
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

/**
 * A token made only of digits is a VALUE, never an intent. In a query it is a
 * dimension the caller will pass as an argument ("a ball 20 across", "bore
 * from 8 to 10"); in a description it is a worked example's number
 * (blackboard_add_entry's "48\cos 20^\circ"). Matching one against the other
 * ranks tools by coincident arithmetic: a rare number in one description
 * carries a high IDF, so "tapered spigot 20 at the base…" put
 * blackboard_add_entry above create_cone on the digit "20" alone (+31.4).
 * Mixed tokens ("m6", "dn50", "h7") keep their letters and still match, and a
 * standard's number is glued to its prefix by PHRASES first ("ISO 2768" →
 * "iso2768"), so it is never mistaken for a bare value.
 */
const PURE_NUMBER = /^[0-9]+$/;

/** Lowercase, rewrite known phrases, split, drop stopwords and bare numbers, stem. */
function tokenize(text: string): string[] {
  let t = text.toLowerCase();
  for (const [re, canon] of PHRASES) t = t.replace(re, canon);
  return t
    .split(/[^a-z0-9]+/)
    // 1-char tokens ("w", "x") are pure noise; bare numbers are values (above).
    .filter((w) => w.length >= 2 && !STOPWORDS.has(w) && !PURE_NUMBER.test(w))
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
function buildIdf(
  table: ToolTable,
  counts: (name: string) => boolean = () => true,
): { nameIdf: Map<string, number>; purposeIdf: Map<string, number> } {
  const nameDf = new Map<string, number>();
  const purposeDf = new Map<string, number>();
  const all = table.all();
  for (const entry of all) {
    // `counts` decides whose NAME tokens are counted (see rankTools); every
    // purpose is counted.
    if (counts(entry.name)) {
      for (const tok of new Set(nameTokensOf(entry.name))) nameDf.set(tok, (nameDf.get(tok) ?? 0) + 1);
    }
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

/**
 * A SHEET verb works on a drawing that already exists: a drawing-bench tool
 * registered experimental (drawing_read_semantics, drawing_move_view, ...),
 * as opposed to the stable drawing tools that start from the part
 * (make_drawing, dimension_part, gdt_fcf). Their names carry generic verbs --
 * move, view, remove, dimension, align -- and a name-token hit outweighs a
 * synonym's, so "move the part 10mm to the left" ranked drawing_move_dimension
 * above transform (2026-11-20). With no SHEET CONTEXT in the query a sheet
 * verb's score is scaled by this factor: it still answers, below the part's
 * own verbs.
 */
const SHEET_VERB_OUT_OF_CONTEXT = 0.5;

/** A sheet verb, by its registration: drawing bench, experimental. */
function isSheetVerb(name: string): boolean {
  const { bench, stability } = metaFor(name);
  return bench === "drawing" && stability === "experimental";
}

/**
 * The words that put a query in a sheet's context: the drawing bench's own
 * name, and every NAME token that only drawing-bench tools carry and that no
 * other tool's name or purpose uses ("sheet", "semantic", "projection", ...).
 * Read from the registry on every search, never listed by hand: a generic
 * verb a part tool also speaks of ("move" is in transform's purpose, "view"
 * in section_view's name) is not context.
 */
function sheetContextTokens(table: ToolTable): Set<string> {
  const drawingNames = new Set<string>();
  const elsewhere = new Set<string>();
  for (const entry of table.all()) {
    const { bench } = metaFor(entry.name);
    if (bench === "drawing") {
      for (const t of nameTokensOf(entry.name)) drawingNames.add(t);
    } else {
      for (const t of nameTokensOf(entry.name)) elsewhere.add(t);
      // A word a part verb (the core bench) speaks of is not sheet context.
      if (bench === "core") for (const t of tokenize(entry.description)) elsewhere.add(t);
    }
  }
  const context = new Set<string>([stem("drawing")]);
  for (const t of drawingNames) if (!elsewhere.has(t)) context.add(t);
  return context;
}

// ─── Meaning in context: the ontology's first slice ──────────────────────────
//
// A generic verb names a different tool in each domain: "solve" is the sketch
// solver in a sketch, the mate solver in an assembly and the study's run in a
// strength study. Ranking words cannot tell them apart -- the domain can. This
// table is that knowledge as DATA (word -> meaning per domain), so it can move
// into the ontology/graph store unchanged; test/find_tool_context.test.mjs
// checks every tool it names exists in the bench it is listed under. `core` is
// the part's own meaning: used when no domain is in use, or when the domain
// gives the verb none.
export const VERB_MEANINGS: Record<string, { words: string[]; means: Partial<Record<Bench, string>> }> = {
  solve: {
    words: ["solve"],
    means: { sketch: "psketch_solve", assembly: "assembly_solve", simulate: "sim_static" },
  },
  run: { words: ["run"], means: { simulate: "sim_static" } },
  verify: {
    words: ["verify", "certify", "validate"],
    means: { core: "verify_part", sketch: "psketch_certify", assembly: "assembly_certify" },
  },
  export: { words: ["export"], means: { core: "export_part", drawing: "drawing_export_sheet" } },
};

/**
 * A word's meaning INSIDE a domain, as the words that domain's tools use for
 * it. In a strength study "pin", "fix" and "constraint" are supports and a
 * "load" is a force; in a sketch "fix" is a constraint; in an assembly "pin"
 * is a mate. Applied only while that domain is the working context: the query
 * gains the domain's own words, so the global vocabulary is untouched with no
 * context. Keys and values are plain words (stemmed when used). Same data
 * shape the ontology will hold.
 */
export const DOMAIN_WORDS: Partial<Record<Bench, Record<string, string[]>>> = {
  simulate: {
    // Not "hold" ("will this hold 500 N" asks for the verdict) and not
    // "clamp" ("an FEA on the clamp" names the part): both read as a
    // support only some of the time.
    pin: ["support"], pinned: ["support"], fix: ["support"], fixed: ["support"],
    bolt: ["support"], anchor: ["support"],
    restrain: ["support"], constraint: ["support"], constrain: ["support"],
    load: ["force"], push: ["force"], pull: ["force"], press: ["force"],
    material: ["material"], alloy: ["material"], steel: ["material"], aluminium: ["material"],
    aluminum: ["material"], titanium: ["material"],
  },
  sketch: {
    fix: ["constrain"], fixed: ["constrain"], lock: ["constrain"], pin: ["constrain"],
    constraint: ["constrain"],
  },
  assembly: {
    pin: ["mate", "revolute"], hinge: ["mate", "revolute"], fix: ["mate"], fixed: ["mate"],
    constraint: ["mate"], constrain: ["mate"],
  },
};

/**
 * Words that mean ONE domain whatever else the query says -- quantities only
 * that domain has. With no domain in use, a query using one is read in that
 * domain ("apply a force on the top face" is study set-up, not a face pick).
 * Deliberately NOT here: "load" (a file is loaded too), "pin" / "fix" /
 * "constraint" (a meaning in three domains), "push" (push-pull modelling).
 */
export const DOMAIN_SIGNALS: Partial<Record<Bench, string[]>> = {
  // ("500 N" and "newtons" reach here as "force", see PHRASES.)
  simulate: ["force", "stress", "fea", "mises"],
};

/** The one domain a query's own words name, with those words; null when none or several. */
export function inferredDomain(query: string): { bench: Bench; words: string[] } | null {
  const toks = new Set(tokenize(query));
  const hits: Array<{ bench: Bench; words: string[] }> = [];
  for (const [b, words] of Object.entries(DOMAIN_SIGNALS) as Array<[Bench, string[]]>) {
    const found = words.filter((w) => toks.has(stem(w)));
    if (found.length) hits.push({ bench: b, words: found });
  }
  return hits.length === 1 ? hits[0] : null;
}

/** Words that add no intent to a bare verb ("solve it again now"). */
const FILLER = new Set(["again", "now", "once", "go", "ahead", "then", "next", "ok", "okay"].map(stem));

/** The order benches are listed in when a word is ambiguous between them. */
const BENCH_ORDER: Bench[] = ["core", "sketch", "assembly", "drawing", "analysis", "simulate", "labels", "timeline", "meta"];

/** The verb a query consists of (filler aside), if it is one of VERB_MEANINGS'. */
function bareVerbOf(query: string): string | null {
  const toks = tokenize(query).filter((t) => !FILLER.has(t));
  if (toks.length !== 1) return null;
  for (const [verb, { words }] of Object.entries(VERB_MEANINGS))
    if (words.some((w) => stem(w) === toks[0])) return verb;
  return null;
}

/**
 * A tool in the working context's bench outranks one outside it by this
 * factor: in a study, "fix the bottom face as the support" is a support
 * (sim_setup), not a face pick. Context lifts; it does not override a
 * clearly stronger match elsewhere ("export the part as stl" stays export_part).
 */
const W_CONTEXT = 1.25;

/**
 * Added to every matching tool of the working bench when the query uses a
 * word that bench defines (DOMAIN_WORDS): larger than any score a query can
 * reach (W_EXACT_NAME is 1000), so those tools form the first group -- a
 * partition, not a tuned weight.
 */
const CLAIMED_OFFSET = 10_000;

/** The domain words a query uses in the working context (for the answer's `context`). */
export function claimedWords(query: string, contextBench: Bench | null): string[] {
  if (contextBench === null) return [];
  const words = DOMAIN_WORDS[contextBench] ?? {};
  const stems = new Set(Object.keys(words).map(stem));
  return [...new Set(tokenize(query).filter((t) => stems.has(t)))];
}

/** Rank the whole table against a query; deterministic total order. */
export function rankTools(
  table: ToolTable,
  query: string,
  benchFilter?: string,
  limit = 5,
  contextBench: Bench | null = null,
): Scored[] {
  const baseTokens = tokenize(query);
  // In a domain, a word also says what the domain's tools call it.
  const domainWords = contextBench !== null && !benchFilter ? DOMAIN_WORDS[contextBench] : undefined;
  const domainStems = new Map<string, string[]>(
    Object.entries(domainWords ?? {}).map(([w, adds]) => [stem(w), adds.map(stem)]),
  );
  const queryTokens = [...baseTokens];
  for (const t of baseTokens) for (const add of domainStems.get(t) ?? []) if (!queryTokens.includes(add)) queryTokens.push(add);
  const claimed = baseTokens.some((t) => domainStems.has(t));
  const queryCreates = queryTokens.some((t) => CREATE_VERB_STEMS.has(t));
  const queryMutates = queryTokens.some((t) => MUTATE_VERB_STEMS.has(t));
  const context = sheetContextTokens(table);
  const sheetContext = queryTokens.some((t) => context.has(t));
  // Out of a sheet's context the sheet verbs are not competitors, so their
  // NAMES do not dilute the rarity of the name tokens they share with the
  // part's verbs either: six layout verbs named "...dimension" / "...view"
  // lowered dimension_part's and section_view's own name-token weight.
  const competes = (name: string) => sheetContext || !!benchFilter || !isSheetVerb(name);
  const { nameIdf, purposeIdf } = buildIdf(table, competes);
  const scored: Scored[] = [];
  for (const entry of table.all()) {
    const { bench } = metaFor(entry.name);
    if (benchFilter && bench !== benchFilter) continue;
    const purposeWords = new Set(tokenize(entry.description));
    let score = scoreTool(entry, queryTokens, queryCreates, queryMutates, purposeWords, nameIdf, purposeIdf);
    // A bench filter is itself the context: the caller asked for the bench.
    if (!sheetContext && !benchFilter && isSheetVerb(entry.name)) score *= SHEET_VERB_OUT_OF_CONTEXT;
    if (contextBench !== null && !benchFilter && bench === contextBench) score *= W_CONTEXT;
    if (score <= 0) continue;
    // A word the working domain defines claims the query for that domain:
    // its tools rank first ("pin the hole" in a study is a support, not a
    // drilling). Ranking within each group is unchanged.
    if (claimed && bench === contextBench) score += CLAIMED_OFFSET;
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

/** Most characters a purpose's gist keeps in a search result. */
const GIST_MAX = 200;
/** A first sentence shorter than this runs on into the next. */
const GIST_MIN_SENTENCE = 30;

/**
 * A purpose's gist: its first sentence, at most GIST_MAX characters (cut at a
 * word, marked with an ellipsis). What a search result or the catalog shows;
 * describe_tool shows the whole purpose.
 */
export function gist(purpose: string, max: number = GIST_MAX): string {
  const text = purpose.trim();
  // A sentence ends at . ! ? before a space or the end -- not after an
  // abbreviation ("e.g.", "i.e.", "vs.", "etc.") or inside a decimal. A first
  // sentence too short to say what the tool does ("THE FULL CERTIFICATE.")
  // runs on into the next.
  let end = -1;
  for (const m of text.matchAll(/[.!?](?=\s|$)/g)) {
    const before = text.slice(0, m.index);
    if (/(\be\.g|\bi\.e|\bvs|\betc|\bcf)$/i.test(before)) continue;
    if (m.index + 1 < GIST_MIN_SENTENCE) continue;
    end = m.index;
    break;
  }
  const first = end >= 0 ? text.slice(0, end + 1) : text;
  if (first.length <= max) return first;
  const cut = first.slice(0, max);
  const space = cut.lastIndexOf(" ");
  return (space > max / 2 ? cut.slice(0, space) : cut).replace(/[\s,;:(—-]+$/, "") + "…";
}

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
        .enum(["core", "sketch", "assembly", "drawing", "analysis", "simulate", "labels", "timeline", "meta"])
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
      const brief = (name: string) => {
        const e = table.get(name);
        return {
          name,
          bench: metaFor(name).bench,
          purpose: gist(e?.description ?? ""),
          token_estimate: e ? estimateTokens(e) : 0,
        };
      };
      const ctx = workingContext();

      // A bare verb ("solve it") means what it means in the working context.
      const verb = bench ? null : bareVerbOf(query);
      if (verb) {
        const means = VERB_MEANINGS[verb].means;
        const inDomain = ctx.bench !== null ? means[ctx.bench] : undefined;
        const pick = inDomain ?? means.core;
        if (pick && table.has(pick)) {
          return okMeta({
            query,
            matches: [brief(pick)],
            resolved_by: inDomain
              ? `working in '${ctx.bench}' (${ctx.from}): '${verb}' there means ${pick}`
              : ctx.bench !== null
                ? `'${verb}' has no meaning in '${ctx.bench}'; the part's own: ${pick}`
                : `no domain in use, so the part's own meaning: ${pick}`,
            next: "invoke({name, args}); describe_tool({name}) for its arguments.",
          });
        }
        const order = BENCH_ORDER.filter((b) => means[b] && table.has(means[b] as string));
        return okMeta({
          query,
          ambiguous: {
            verb,
            means: Object.fromEntries(order.map((b) => [b, means[b]])),
            resolve: `'${verb}' means a different tool in each domain and none is in use: enter one (workbench('${order[0]}')) or name it in the query (e.g. '${verb} the ${order[0]}').`,
          },
          matches: order.map((b) => brief(means[b] as string)),
        });
      }

      // No domain in use: the query's own single-meaning words may name one.
      const inferred = ctx.bench === null && !bench ? inferredDomain(query) : null;
      const contextBench = ctx.bench ?? inferred?.bench ?? null;
      const results = rankTools(table, query, bench, limit ?? 5, contextBench);
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
      const read = bench ? [] : claimedWords(query, contextBench);
      return okMeta({
        query,
        ...(contextBench !== null && !bench
          ? {
              context: {
                working_in: contextBench,
                ...(inferred ? { inferred_from: inferred.words } : { from: ctx.from }),
                ...(read.length ? { domain_words: read } : {}),
              },
            }
          : {}),
        // The ranking reads every word of each purpose; the RESULT carries
        // only its gist -- a search that returned five full purposes cost up
        // to 2.6k tokens, mostly a workflow repeated per tool. describe_tool
        // has the whole text.
        matches: results.map((r) => ({
          name: r.name,
          bench: r.bench,
          purpose: gist(r.purpose),
          token_estimate: r.token_estimate,
        })),
        next: "describe_tool({name}) for the full purpose and schema, then invoke({name, args}) to run it. workbench('catalog') lists every tool by bench.",
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
        // A backend-defined tool also says what it does to the model and
        // shows worked calls, straight from its one definition (served.ts).
        ...((entry.served as any)?.examples
          ? {
              effects: (entry.served as any).effects,
              examples: (entry.served as any).examples,
            }
          : {}),
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
