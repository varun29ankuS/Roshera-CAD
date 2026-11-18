// registry schema parity — do the two copies of each tool's PARAMETERS agree?
//
// Roshera states every kernel tool twice: the zod schema the MCP server
// registers (src/tools/*.ts, compiled into test/.build) and the transcribed
// JSON Schema row in roshera-backend/api-server/src/agent_registry.rs (served
// at GET /api/agent/tool-registry). The ontology gate (ontology_coverage.mjs
// o5) and the registry hash compare NAMES and benches only; parameters drifted
// underneath them (the MCP audit of 2026-11-17 measured 17 of 115 tools).
//
// What is compared, for every tool present on BOTH sides, at the top level of
// the tool's input object (the level an agent fills in):
//
//   param     a property exists on one side only.        side = the side that HAS it
//   required  a shared property is required on one side. side = the side that REQUIRES it
//   type      a shared property's JSON type set differs,  side = "both"
//             when BOTH sides declare one. The type set is the union of `type`
//             over the schema and its anyOf/oneOf members (enum/const values
//             contribute their JSON type); a schema with no determinable type
//             (no `type`, an unresolvable $ref, `{}`; local `#/…` refs are
//             followed) is "undeclared" and is not compared, so
//             an untyped z.any() on one side is not reported as a type drift.
//             `integer` and `number` are distinct (zod `.int()` emits integer;
//             a backend `number` there accepts 2.5 the MCP would refuse).
//
// Not compared (deliberately, so the list stays about the contract an agent
// sends): descriptions, defaults, numeric bounds, enum value lists, nested
// object properties. Those are Stage 1's job (one schema, derived), not a
// text-diff's.
//
// Tools on ONE side only are reported, not failed: the name gate owns that.
//
// The ratchet: every difference must be listed in
// registry_schema_parity.allow.json ({tool, kind, field, side, reason}), and
// every listed entry must still be a live difference. A new drift fails; a
// closed drift whose line was not deleted fails. The list can only shrink —
// the same rule as geometry-engine/KNOWN_REDS.md.
//
// The Rust row parser is a small evaluator for the subset of `json!` the table
// uses (objects, arrays, string/number/bool/null literals, and the two schema
// helpers `ntuple` / `plane_schema`, whose bodies are read from the same
// source). Anything else is refused by line number rather than guessed, and
// the row count is cross-checked against the file's own `assert_eq!(tools.len(), N)`.
//
// Run: npm run test:gates:build && node test/registry_schema_parity.mjs
// Overrides (for proving the gate RED on scratch copies, never needed in CI):
//   PARITY_RUST_SOURCE=<path to an agent_registry.rs copy>
//   PARITY_ALLOW=<path to an allow-list copy>
// Exit 0 = pass, 1 = fail.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { buildTable } from "./.build/surface.js";
import { toolJsonSchema } from "./.build/registry.js";
import { loadCheckedInDefs } from "./.build/served.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const RUST_PATH =
  process.env.PARITY_RUST_SOURCE ??
  path.join(HERE, "..", "..", "roshera-backend", "api-server", "src", "agent_registry.rs");
const ALLOW_PATH = process.env.PARITY_ALLOW ?? path.join(HERE, "registry_schema_parity.allow.json");
// The MCP-side funnel tools the backend registry does not curate by design.
const MCP_FUNNEL = new Set(["find_tool", "describe_tool", "invoke"]);

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);

// ─── Rust `json!` subset evaluator ──────────────────────────────────────────

class RustParseError extends Error {}

function lineOf(src, at) {
  let n = 1;
  for (let i = 0; i < at && i < src.length; i++) if (src[i] === "\n") n++;
  return n;
}

/** A cursor over Rust source with the lexical pieces the table uses. */
function cursor(src, start, end, lineBase) {
  let i = start;
  const err = (msg) => {
    throw new RustParseError(`agent_registry.rs:${lineBase(i)}: ${msg}`);
  };
  const ws = () => {
    for (;;) {
      while (i < end && /\s/.test(src[i])) i++;
      if (src.startsWith("//", i)) {
        while (i < end && src[i] !== "\n") i++;
      } else if (src.startsWith("/*", i)) {
        const close = src.indexOf("*/", i + 2);
        if (close < 0) err("unterminated block comment");
        i = close + 2;
      } else return;
    }
  };
  const peek = () => {
    ws();
    return src[i];
  };
  const expect = (tok) => {
    ws();
    if (!src.startsWith(tok, i)) err(`expected '${tok}', found '${src.slice(i, i + 20)}'`);
    i += tok.length;
  };
  const tryTok = (tok) => {
    ws();
    if (src.startsWith(tok, i)) {
      i += tok.length;
      return true;
    }
    return false;
  };
  const ident = () => {
    ws();
    const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i, i + 80));
    if (!m) err(`expected an identifier, found '${src.slice(i, i + 20)}'`);
    i += m[0].length;
    return m[0];
  };
  const string = () => {
    ws();
    const raw = /^r(#*)"/.exec(src.slice(i, i + 12));
    if (raw) {
      const close = '"' + raw[1];
      const from = i + raw[0].length;
      const to = src.indexOf(close, from);
      if (to < 0) err("unterminated raw string");
      i = to + close.length;
      return src.slice(from, to);
    }
    if (src[i] !== '"') err(`expected a string literal, found '${src.slice(i, i + 20)}'`);
    i++;
    let out = "";
    while (i < end) {
      const c = src[i];
      if (c === '"') {
        i++;
        return out;
      }
      if (c !== "\\") {
        out += c;
        i++;
        continue;
      }
      const e = src[i + 1];
      if (e === "\n" || e === "\r") {
        i += 1;
        while (i < end && /\s/.test(src[i])) i++; // line continuation
        continue;
      }
      const simple = { n: "\n", t: "\t", r: "\r", "\\": "\\", '"': '"', "'": "'", 0: "\0" };
      if (e in simple) {
        out += simple[e];
        i += 2;
        continue;
      }
      if (e === "u") {
        const m = /^\\u\{([0-9a-fA-F]{1,6})\}/.exec(src.slice(i, i + 12));
        if (!m) err("malformed \\u{…} escape");
        out += String.fromCodePoint(parseInt(m[1], 16));
        i += m[0].length;
        continue;
      }
      err(`unsupported string escape '\\${e}'`);
    }
    err("unterminated string literal");
  };
  return {
    get pos() {
      return i;
    },
    set pos(v) {
      i = v;
    },
    ws,
    peek,
    expect,
    tryTok,
    ident,
    string,
    err,
    slice: (a, b) => src.slice(a, b),
  };
}

/**
 * Parse one `json!` value at the cursor. `env` binds identifiers (helper
 * parameters); `helpers` evaluates helper calls. Rust `json!` allows any
 * expression as a value; this accepts only what the table actually uses and
 * refuses the rest by line, so a new construct can never be silently misread.
 */
function parseValue(c, env, helpers) {
  const ch = c.peek();
  if (ch === "{") {
    c.expect("{");
    const obj = {};
    while (!c.tryTok("}")) {
      const key = c.string();
      c.expect(":");
      if (Object.prototype.hasOwnProperty.call(obj, key)) c.err(`duplicate key "${key}"`);
      obj[key] = parseValue(c, env, helpers);
      if (!c.tryTok(",")) {
        c.expect("}");
        break;
      }
    }
    return obj;
  }
  if (ch === "[") {
    c.expect("[");
    const arr = [];
    while (!c.tryTok("]")) {
      arr.push(parseValue(c, env, helpers));
      if (!c.tryTok(",")) {
        c.expect("]");
        break;
      }
    }
    return arr;
  }
  if (ch === '"' || (ch === "r" && /^r#*"/.test(c.slice(c.pos, c.pos + 8)))) return c.string();
  if (ch === "-" || /[0-9]/.test(ch)) {
    const m = /^-?[0-9][0-9_]*(\.[0-9_]+)?([eE][+-]?[0-9]+)?/.exec(c.slice(c.pos, c.pos + 40));
    if (!m) c.err("malformed number");
    c.pos += m[0].length;
    if (/^[A-Za-z]/.test(c.slice(c.pos, c.pos + 1))) c.err("numeric literal with a type suffix is not supported");
    return Number(m[0].replace(/_/g, ""));
  }
  const name = c.ident();
  if (name === "true") return true;
  if (name === "false") return false;
  if (name === "null") return null;
  if (name === "json" && c.tryTok("!")) {
    c.expect("(");
    const v = parseValue(c, env, helpers);
    c.expect(")");
    return v;
  }
  if (c.peek() === "(") {
    c.expect("(");
    const args = [];
    while (!c.tryTok(")")) {
      args.push(parseValue(c, env, helpers));
      if (!c.tryTok(",")) {
        c.expect(")");
        break;
      }
    }
    if (!helpers.has(name)) c.err(`call to '${name}(…)' inside a schema: no evaluator for it — extend the parser`);
    return helpers.get(name)(args, c);
  }
  if (Object.prototype.hasOwnProperty.call(env, name)) return env[name];
  c.err(`identifier '${name}' in a schema value: not a literal, a bound helper parameter, or a helper call`);
}

/** Locate `fn <name>(` and return the index of its body's opening brace. */
function fnBody(src, name) {
  const at = src.search(new RegExp(`\\bfn\\s+${name}\\s*\\(`));
  if (at < 0) throw new RustParseError(`agent_registry.rs: fn ${name} not found`);
  const open = src.indexOf("{", at);
  return { at, open };
}

function parseRegistry(src) {
  const lineBase = (pos) => lineOf(src, pos);
  const helpers = new Map();

  // ntuple(n, desc): its body is a single json! whose values reference n/desc.
  {
    const { open } = fnBody(src, "ntuple");
    const jsonAt = src.indexOf("json!(", open);
    const params = ["n", "desc"];
    helpers.set("ntuple", (args, at) => {
      if (args.length !== 2) at.err(`ntuple takes 2 arguments, got ${args.length}`);
      const c = cursor(src, jsonAt + "json!(".length, src.length, lineBase);
      return parseValue(c, Object.fromEntries(params.map((p, k) => [p, args[k]])), new Map());
    });
  }
  // plane_schema(default_xy): `let mut v = json!(…)`, then `default: "xy"`
  // inserted when the flag is set. The insertion is matched literally so a
  // change to the helper's shape refuses instead of being half-modelled.
  {
    const { open } = fnBody(src, "plane_schema");
    const close = src.indexOf("\n}", open);
    const body = src.slice(open, close);
    const jsonAt = src.indexOf("json!(", open);
    const insert = /if\s+default_xy\s*\{[\s\S]*?obj\.insert\(\s*"default"\.to_string\(\),\s*json!\(\s*"xy"\s*\)\s*\)/;
    if (!insert.test(body))
      throw new RustParseError(
        `agent_registry.rs:${lineBase(open)}: plane_schema no longer has the default_xy → default:"xy" insertion this parser models — update the parser`,
      );
    helpers.set("plane_schema", (args, at) => {
      if (args.length !== 1 || typeof args[0] !== "boolean") at.err("plane_schema takes one bool");
      const c = cursor(src, jsonAt + "json!(".length, src.length, lineBase);
      const v = parseValue(c, {}, new Map());
      if (args[0]) v.default = "xy";
      return v;
    });
  }

  const { open } = fnBody(src, "raw_tools");
  const vecAt = src.indexOf("vec![", open);
  if (vecAt < 0) throw new RustParseError("agent_registry.rs: raw_tools has no vec![");
  const c = cursor(src, vecAt + "vec![".length, src.length, lineBase);
  const rows = new Map();
  while (!c.tryTok("]")) {
    const rowAt = c.pos;
    if (c.ident() !== "t") c.err("expected a t(…) row");
    c.expect("(");
    const name = c.string();
    c.expect(",");
    const bench = c.ident();
    c.expect(",");
    const stability = c.ident();
    c.expect(",");
    const source = c.ident();
    c.expect(",");
    c.string(); // purpose
    c.expect(",");
    c.expect("json!(");
    const schema = parseValue(c, {}, helpers);
    c.expect(")");
    c.tryTok(",");
    c.expect(")");
    if (rows.has(name)) c.err(`duplicate backend row '${name}'`);
    rows.set(name, { name, bench, stability, source, schema, line: lineBase(rowAt) });
    if (!c.tryTok(",")) {
      c.expect("]");
      break;
    }
  }
  return rows;
}

// ─── Comparison ─────────────────────────────────────────────────────────────

const jsonType = (v) =>
  v === null ? "null" : Array.isArray(v) ? "array" : Number.isInteger(v) ? "integer" : typeof v;

/**
 * Follow a local JSON pointer (`#/properties/a`) — zod-to-json-schema emits one
 * when two fields share a zod object (assembly_mate's `b` reuses `a`).
 * Unresolvable or remote refs stay unresolved (the field counts as untyped).
 */
function deref(s, root, seen = new Set()) {
  while (s && typeof s === "object" && typeof s.$ref === "string" && s.$ref.startsWith("#/")) {
    if (seen.has(s.$ref)) return null;
    seen.add(s.$ref);
    let t = root;
    for (const part of s.$ref.slice(2).split("/")) {
      const k = part.replace(/~1/g, "/").replace(/~0/g, "~");
      t = t && typeof t === "object" ? t[k] : undefined;
    }
    if (t === undefined) return null;
    s = t;
  }
  return s;
}

/** The set of JSON types a schema admits, or null when it does not say. */
function typeSet(s, root) {
  s = deref(s, root);
  if (s === null || typeof s !== "object") return null;
  if (typeof s.type === "string") return new Set([s.type]);
  if (Array.isArray(s.type)) return new Set(s.type);
  const union = s.anyOf ?? s.oneOf;
  if (Array.isArray(union)) {
    const out = new Set();
    for (const m of union) {
      const t = typeSet(m, root);
      if (t === null) return null;
      for (const x of t) out.add(x);
    }
    return out;
  }
  if (Array.isArray(s.enum) && s.enum.length) {
    const out = new Set(s.enum.map(jsonType));
    // a numeric enum mixing ints and floats is "number"
    if (out.has("integer") && out.has("number")) out.delete("integer");
    return out;
  }
  if ("const" in s) return new Set([jsonType(s.const)]);
  return null;
}
const fmtTypes = (t) => [...t].sort().join("|");

function topLevel(schema) {
  const props = schema && typeof schema.properties === "object" ? schema.properties : {};
  const required = new Set(Array.isArray(schema?.required) ? schema.required : []);
  return { props, required };
}

/** Every difference between one tool's two schemas, as ratchet keys. */
function diffTool(name, mcpSchema, backSchema) {
  const out = [];
  const m = topLevel(mcpSchema);
  const b = topLevel(backSchema);
  const names = new Set([...Object.keys(m.props), ...Object.keys(b.props)]);
  for (const field of [...names].sort()) {
    const inM = Object.prototype.hasOwnProperty.call(m.props, field);
    const inB = Object.prototype.hasOwnProperty.call(b.props, field);
    if (inM !== inB) {
      out.push({ tool: name, kind: "param", field, side: inM ? "mcp" : "backend" });
      continue;
    }
    const rM = m.required.has(field);
    const rB = b.required.has(field);
    if (rM !== rB) out.push({ tool: name, kind: "required", field, side: rM ? "mcp" : "backend" });
    const tM = typeSet(m.props[field], mcpSchema);
    const tB = typeSet(b.props[field], backSchema);
    STATS.shared += 1;
    if (rM || rB) STATS.requiredAnywhere += 1;
    if (tM && tB) STATS.typeCompared += 1;
    else if (!tM && !tB) STATS.untypedBoth.push(`${name}.${field}`);
    else STATS.untypedOne.push(`${name}.${field} (${tM ? "backend" : "mcp"} untyped)`);
    if (tM && tB && fmtTypes(tM) !== fmtTypes(tB))
      out.push({
        tool: name,
        kind: "type",
        field,
        side: "both",
        detail: `mcp=${fmtTypes(tM)} backend=${fmtTypes(tB)}`,
      });
  }
  // `required` naming a property that does not exist on that side is its own
  // defect (a schema that can never be satisfied); report it under `required`.
  for (const [side, t] of [
    ["mcp", m],
    ["backend", b],
  ])
    for (const r of t.required)
      if (!Object.prototype.hasOwnProperty.call(t.props, r))
        out.push({ tool: name, kind: "required", field: r, side, detail: "required but not a property" });
  return out;
}

// Coverage counters, printed so a pass can be read as "compared N things",
// not mistaken for "compared nothing".
const STATS = { shared: 0, requiredAnywhere: 0, typeCompared: 0, untypedBoth: [], untypedOne: [] };

const keyOf = (d) => `${d.tool} | ${d.kind} | ${d.field} | ${d.side}${d.detail ? " | " + d.detail : ""}`;

// ─── Run ────────────────────────────────────────────────────────────────────

console.log("registry schema parity — MCP zod (test/.build) vs agent_registry.rs rows");
console.log(`  backend source: ${RUST_PATH}`);
console.log(`  allow list:     ${ALLOW_PATH}`);

let rows = null;
try {
  rows = parseRegistry(readFileSync(RUST_PATH, "utf8"));
} catch (e) {
  fail(`cannot parse the backend registry: ${e.message}`);
}

let allow = null;
try {
  allow = JSON.parse(readFileSync(ALLOW_PATH, "utf8"));
} catch (e) {
  fail(`cannot read the allow list: ${e.message}`);
}

if (rows !== null && allow !== null) {
  // (p1) parser completeness against the file's own count assertion.
  const src = readFileSync(RUST_PATH, "utf8");
  const countMatch = src.match(/assert_eq!\(\s*tools\.len\(\),\s*(\d+)/);
  if (!countMatch) fail("no 'assert_eq!(tools.len(), N' literal in agent_registry.rs — cannot prove the parse is complete");
  else if (Number(countMatch[1]) !== rows.size)
    fail(`parsed ${rows.size} backend rows but agent_registry.rs asserts ${countMatch[1]} — parser out of step`);
  else pass(`parsed ${rows.size} backend rows (== the file's tools.len() assertion)`);

  // (p1b) the backend-owned definitions (agent_tools/*.json) are the backend
  // copy for the tools they define — served by `agent_registry::build_registry`
  // from the same file. A tool may have ONE backend copy: a table row AND a
  // definition would be the drift this design exists to end.
  const servedDefs = loadCheckedInDefs().tools;
  for (const def of servedDefs) {
    if (rows.has(def.name)) fail(`${def.name} has a table row in agent_registry.rs AND a backend definition`);
    rows.set(def.name, { name: def.name, schema: def.input_schema, source: "served" });
  }
  pass(`${servedDefs.length} backend-defined tools read from agent_tools/*.json`);

  // (p2) shared tools, one-sided tools (reported only).
  const table = buildTable();
  const mcpNames = table.names().filter((n) => !MCP_FUNNEL.has(n));
  const shared = mcpNames.filter((n) => rows.has(n)).sort();
  const onlyMcp = mcpNames.filter((n) => !rows.has(n)).sort();
  const onlyBackend = [...rows.keys()].filter((n) => !table.has(n)).sort();
  console.log(`  MCP tools ${mcpNames.length} (+${MCP_FUNNEL.size} funnel), backend rows ${rows.size}, compared ${shared.length}`);
  console.log(`  only in MCP (not compared; the name gate owns this): ${onlyMcp.join(", ") || "none"}`);
  console.log(`  only in backend (not compared; the name gate owns this): ${onlyBackend.join(", ") || "none"}`);
  if (shared.length === 0) fail("no tool is present on both sides — the comparison would be vacuous");

  // (p3) every MCP schema must actually be an object schema with properties;
  // toolJsonSchema degrades to {type:"object"} on a conversion error, which
  // would make every backend param look backend-only. Refuse that silently.
  const diffs = [];
  for (const name of shared) {
    const js = toolJsonSchema(table.get(name));
    const back = rows.get(name).schema;
    if (js.type !== "object" || back.type !== "object") {
      fail(`${name}: input schema is not an object on both sides (mcp=${js.type}, backend=${back.type})`);
      continue;
    }
    if (js.properties === undefined && back.properties !== undefined && Object.keys(back.properties).length)
      fail(`${name}: MCP schema has no properties at all — zod→JSON conversion likely failed; not comparing it`);
    diffs.push(...diffTool(name, js, back));
  }
  const toolsWithDiff = new Set(diffs.map((d) => d.tool));
  console.log(
    `  shared parameters ${STATS.shared}: required on some side ${STATS.requiredAnywhere}, ` +
      `type compared ${STATS.typeCompared}, untyped on both ${STATS.untypedBoth.length}, untyped on one ${STATS.untypedOne.length}`,
  );
  if (STATS.untypedOne.length) console.log(`    type not compared (one side untyped): ${STATS.untypedOne.join(", ")}`);
  if (STATS.untypedBoth.length) console.log(`    type not compared (both untyped): ${STATS.untypedBoth.join(", ")}`);
  if (STATS.shared > 0 && STATS.typeCompared === 0) fail("no parameter type was compared — the type normaliser is broken");
  console.log(`  differences measured: ${diffs.length} across ${toolsWithDiff.size} of ${shared.length} tools`);

  // (p4) allow-list hygiene.
  const ALLOWED_KEYS = new Set(["tool", "kind", "field", "side", "detail", "reason"]);
  const SIDES = { param: ["mcp", "backend"], required: ["mcp", "backend"], type: ["both"] };
  const allowed = new Map();
  const entries = Array.isArray(allow.differences) ? allow.differences : null;
  if (entries === null) fail("allow list must be an object with a `differences` array");
  for (const e of entries ?? []) {
    const extra = Object.keys(e).filter((k) => !ALLOWED_KEYS.has(k));
    if (extra.length) fail(`allow entry ${JSON.stringify(e)} has unknown keys ${extra}`);
    if (!e.tool || !e.kind || !e.field || !e.side) {
      fail(`allow entry ${JSON.stringify(e)} lacks tool/kind/field/side`);
      continue;
    }
    if (!SIDES[e.kind]?.includes(e.side)) fail(`allow entry '${keyOf(e)}': kind '${e.kind}' takes side ${SIDES[e.kind] ?? "?"}`);
    if (typeof e.reason !== "string" || e.reason.trim().length < 10)
      fail(`allow entry '${keyOf(e)}' needs a one-line reason`);
    const k = keyOf(e);
    if (allowed.has(k)) fail(`allow entry '${k}' is listed twice`);
    allowed.set(k, e);
  }

  // (p5) the ratchet, both directions.
  const live = new Map(diffs.map((d) => [keyOf(d), d]));
  const unlisted = [...live.keys()].filter((k) => !allowed.has(k));
  const closed = [...allowed.keys()].filter((k) => !live.has(k));
  for (const k of unlisted)
    fail(`NEW DRIFT  ${k}  — fix the copy that is wrong, or (with a reason) add it to the allow list`);
  for (const k of closed)
    fail(`RATCHET    ${k}  — listed, but no longer a difference: delete this entry from the allow list`);
  if (!unlisted.length && !closed.length)
    pass(`every difference is listed and every listed entry is live (${allowed.size} entries)`);
  for (const k of [...live.keys()].filter((k) => allowed.has(k))) console.log(`      allowed  ${k}`);
}

if (failures) {
  console.error(`\nregistry schema parity: FAIL (${failures})`);
  process.exit(1);
}
console.log("\nregistry schema parity: PASS");
