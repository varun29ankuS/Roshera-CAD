// Slice 2 — MCP scale architecture: meta-tools + minimal default surface.
//
// Node-runnable WITHOUT a live backend. Exercises the compiled dist/ directly
// (build first: `npm run build`). Four groups, matching the S2 spec:
//   (a) validation parity  — invoke(create_box, bad) === direct-call error;
//                            invoke(create_box, good) produces the identical POST.
//   (b) surface flip       — minimal exposes exactly 18; full exposes 90.
//   (c) find_tool ranking  — intent queries surface the right tools top-3.
//   (d) hash vector        — TS FNV-1a-64 matches pinned reference vectors.
//
// Run: node test/scale_s2.mjs   (exit 0 = pass, non-zero = fail)

import { McpError, ErrorCode } from "@modelcontextprotocol/sdk/types.js";
import {
  normalizeObjectSchema,
  safeParseAsync,
  getParseErrorMessage,
} from "@modelcontextprotocol/sdk/server/zod-compat.js";

import {
  buildTable,
  MINIMAL_SURFACE,
  CORE_SURFACE,
  META_SURFACE,
  exposedNamesFor,
  billFor,
} from "../dist/surface.js";
import { rankTools } from "../dist/metatools.js";
import { fnv1a64hex, canonicalJson } from "../dist/registry.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);

// ── (d) FNV-1a-64 hash vectors (pure) ────────────────────────────────────────
console.log("(d) HASH: TS FNV-1a-64 reproduces canonical reference vectors");
{
  // Canonical published FNV-1a-64 test vectors (authoritative). Prove the TS
  // implementation uses the identical algorithm + constants the kernel's
  // fnv1a_64 does (offset 0xcbf29ce484222325, prime 0x100000001b3).
  const VECTORS = [
    ["", "cbf29ce484222325"],
    ["a", "af63dc4c8601ec8c"],
    ["foobar", "85944171f73967e8"],
  ];
  for (const [input, expected] of VECTORS) {
    const got = fnv1a64hex(input);
    if (got === expected) pass(`fnv1a64(${JSON.stringify(input)}) = ${expected}`);
    else fail(`fnv1a64(${JSON.stringify(input)}) = ${got}, expected ${expected}`);
  }

  // canonicalJson sorts keys at every level, compact — byte-compatible with
  // serde_json over a BTreeMap (the form the kernel hashes).
  const canon = canonicalJson({ b: 2, a: 1, c: [3, { y: 2, x: 1 }] });
  if (canon === '{"a":1,"b":2,"c":[3,{"x":1,"y":2}]}')
    pass("canonicalJson sorts keys recursively, compact");
  else fail(`canonicalJson wrong: ${canon}`);

  // Independent BigInt reference of FNV-1a-64 — the "Rust-computed vector by
  // hand from the algorithm". Cross-checks the production impl on a registry-
  // shaped fixture and pins the value.
  const refFnv = (s) => {
    const MASK = (1n << 64n) - 1n;
    let h = 0xcbf29ce484222325n;
    for (const byte of new TextEncoder().encode(s)) {
      h ^= BigInt(byte);
      h = (h * 0x100000001b3n) & MASK;
    }
    return h.toString(16).padStart(16, "0");
  };
  const fixture = [
    { name: "create_box", bench: "core", token_estimate: 120, stability: "stable" },
  ];
  const canonFixture = canonicalJson(fixture);
  const prod = fnv1a64hex(canonFixture);
  const ref = refFnv(canonFixture);
  const PINNED = "0420e165a6be5c98";
  if (prod === ref && prod === PINNED)
    pass(`registry-shaped fixture hash = ${prod} (production == independent reference == pinned)`);
  else
    fail(`fixture hash mismatch: production=${prod} reference=${ref} pinned=${PINNED}`);
}

// ── (b) Surface flip (pure) ──────────────────────────────────────────────────
console.log("(b) SURFACE: minimal exposes 18, full exposes 90");
const table = buildTable();
{
  if (table.size === 93) pass("table holds 93 tools (90 kernel + 3 meta)");
  else fail(`table size ${table.size}, expected 93`);

  const minimal = exposedNamesFor(table, "minimal");
  if (minimal.length === 18) pass("minimal surface exposes exactly 18 tools");
  else fail(`minimal surface exposes ${minimal.length}, expected 18`);

  const minimalSet = new Set(minimal);
  const expectedSet = new Set(MINIMAL_SURFACE);
  if (minimal.length === expectedSet.size && [...expectedSet].every((n) => minimalSet.has(n)))
    pass("minimal surface = the 15 core + 3 meta names exactly");
  else fail(`minimal surface names differ: ${minimal.join(",")}`);

  if (CORE_SURFACE.length === 15) pass("core list is 15 tools");
  else fail(`core list is ${CORE_SURFACE.length}, expected 15`);
  if (META_SURFACE.length === 3) pass("meta list is 3 tools");
  else fail(`meta list is ${META_SURFACE.length}, expected 3`);

  const full = exposedNamesFor(table, "full");
  if (full.length === 90) pass("full surface exposes exactly 90 tools (meta excluded)");
  else fail(`full surface exposes ${full.length}, expected 90`);
  if (!full.some((n) => META_SURFACE.includes(n)))
    pass("full surface omits the meta-tools (they are the minimal-surface mechanism)");
  else fail("full surface unexpectedly includes meta-tools");

  // The MEASURE — bills. Minimal target <5k tokens; full is the worst-client bill.
  const minimalBill = billFor(table, MINIMAL_SURFACE);
  const fullBill = billFor(table, table.names());
  console.log(`      token bill: minimal=${minimalBill}, full=${fullBill}`);
  if (minimalBill < 5000) pass(`minimal token bill ${minimalBill} < 5000 target`);
  else fail(`minimal token bill ${minimalBill} exceeds the 5000 target`);
  if (fullBill > minimalBill) pass(`full bill ${fullBill} > minimal ${minimalBill} (the flip pays off)`);
  else fail("full bill is not greater than minimal — measurement broken");
}

// ── (c) find_tool ranking (pure) ─────────────────────────────────────────────
console.log("(c) FIND_TOOL: intent queries surface the right tools top-3");
{
  const top3 = (q) => rankTools(table, q, undefined, 5).slice(0, 3).map((r) => r.name);

  const drill = top3("drill a bolt circle");
  if (drill.includes("drill_pattern")) pass(`'drill a bolt circle' top-3 = [${drill}] includes drill_pattern`);
  else fail(`'drill a bolt circle' top-3 = [${drill}] missing drill_pattern`);

  const shot = top3("screenshot the scene");
  const hasScene = shot.includes("scene_view");
  const hasRender = shot.includes("render_part");
  if (hasScene && hasRender) pass(`'screenshot the scene' top-3 = [${shot}] includes scene_view + render_part`);
  else fail(`'screenshot the scene' top-3 = [${shot}] missing scene_view/render_part`);

  // Zero-hit honesty: an unmatchable query returns empty, and find_tool says so.
  const none = rankTools(table, "zzqxwv nonsense gibberish", undefined, 5);
  if (none.length === 0) pass("unmatchable query ranks to empty (honest zero-hit)");
  else fail(`unmatchable query returned ${none.length} results: ${none.map((r) => r.name)}`);

  const findTool = table.get("find_tool");
  const emptyRes = await findTool.handler({ query: "zzqxwv nonsense gibberish" });
  const emptyText = emptyRes.content[0].text;
  if (emptyText.includes("Broaden") || emptyText.includes("broaden"))
    pass("find_tool zero-hit result suggests broadening the query");
  else fail(`find_tool zero-hit missing broaden suggestion: ${emptyText}`);
}

// ── (a) Validation parity + dispatch parity (mock the api()/fetch layer) ─────
console.log("(a) INVOKE: validation parity + identical POST body vs direct call");
{
  const entry = table.get("create_box");
  const invoke = table.get("invoke");

  // -- validation parity: bad args produce the identical typed error --
  // `.strict()` on create_box rejects the unknown key `bogus`.
  const badArgs = { width: 10, depth: 10, height: 5, bogus: 1 };

  // Independent "direct call" oracle: exactly what the SDK's validateToolInput
  // does — same normalize + parse + message template + McpError wrapping.
  async function directCallError(schema, args, name) {
    const obj = normalizeObjectSchema(schema);
    const parsed = await safeParseAsync(obj ?? schema, args);
    if (parsed.success) return null;
    const err = "error" in parsed ? parsed.error : "Unknown error";
    const msg = getParseErrorMessage(err);
    return new McpError(
      ErrorCode.InvalidParams,
      `Input validation error: Invalid arguments for tool ${name}: ${msg}`,
    );
  }

  const oracleErr = await directCallError(entry.schema, badArgs, "create_box");
  let invokeErr = null;
  try {
    await invoke.handler({ name: "create_box", args: badArgs });
  } catch (e) {
    invokeErr = e;
  }
  if (!oracleErr) {
    fail("oracle expected create_box bad args to fail validation, but it passed");
  } else if (!invokeErr) {
    fail("invoke did NOT reject bad create_box args (validation parity broken)");
  } else if (invokeErr instanceof McpError && invokeErr.code === oracleErr.code && invokeErr.message === oracleErr.message) {
    pass("invoke bad-args error deep-equals the direct-call typed error (code + message)");
  } else {
    fail(`invoke error ≠ direct error:\n    invoke: ${invokeErr.code} ${invokeErr.message}\n    direct: ${oracleErr.code} ${oracleErr.message}`);
  }

  // -- dispatch parity: good args → identical POST body via invoke vs direct --
  process.env.ROSHERA_MCP_AUTOVERIFY = "0"; // skip ambient perception fetches
  const captured = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    const method = init.method ?? "GET";
    const u = String(url);
    let body = null;
    if (init.body) {
      try { body = JSON.parse(init.body); } catch { body = init.body; }
    }
    captured.push({ method, url: u, body });
    // Canned responses for the create_box call chain.
    let payload = null;
    if (method === "POST" && u.includes("/api/geometry/box")) payload = { object: { id: "uuid-box-1" } };
    else if (method === "GET" && u.endsWith("/api/agent/parts")) payload = [{ id: 1 }];
    else if (method === "GET" && u.includes("/api/agent/parts/")) payload = { location: { center_world: [0, 0, 2.5], dimensions_world: [10, 20, 5] } };
    else payload = {};
    const text = JSON.stringify(payload);
    return { ok: true, status: 200, text: async () => text };
  };

  const goodArgs = { width: 10, depth: 20, height: 5 };
  const boxBodyOf = () => {
    const rec = captured.find((c) => c.method === "POST" && c.url.includes("/api/geometry/box"));
    return rec ? rec.body : null;
  };

  try {
    // Direct call: SDK parses args through the schema, then calls the handler.
    const obj = normalizeObjectSchema(entry.schema);
    const parsedDirect = (await safeParseAsync(obj ?? entry.schema, goodArgs)).data;
    captured.length = 0;
    const directRes = await entry.handler(parsedDirect);
    const directBody = boxBodyOf();

    // Invoke: parses internally through the SAME schema, then dispatches.
    captured.length = 0;
    const invokeRes = await invoke.handler({ name: "create_box", args: goodArgs });
    const invokeBody = boxBodyOf();

    if (directBody && invokeBody && eq(directBody, invokeBody))
      pass(`invoke POST body === direct POST body: ${JSON.stringify(invokeBody)}`);
    else
      fail(`POST body differs:\n    direct: ${JSON.stringify(directBody)}\n    invoke: ${JSON.stringify(invokeBody)}`);

    // Defaults must be applied by invoke (plane→xy ⇒ u_axis=[1,0,0], base at origin).
    if (invokeBody && eq(invokeBody.u_axis, [1, 0, 0]) && eq(invokeBody.center, [0, 0, 0]))
      pass("invoke applied schema defaults before dispatch (plane=xy, base at origin)");
    else
      fail(`invoke did not apply defaults: ${JSON.stringify(invokeBody)}`);

    // Identical tool result too (same handler, same canned backend).
    if (eq(directRes, invokeRes)) pass("invoke result === direct result (same handler dispatched)");
    else fail("invoke result differs from direct result");
  } finally {
    globalThis.fetch = realFetch;
    delete process.env.ROSHERA_MCP_AUTOVERIFY;
  }

  // -- honesty: invoke on an unknown name → typed error naming nearest matches --
  const unknownRes = await invoke.handler({ name: "dril", args: {} });
  const unknownText = unknownRes?.content?.[0]?.text ?? "";
  if (unknownRes?.isError && unknownText.includes("drill_pattern"))
    pass("invoke(unknown) returns a typed error naming the nearest match (drill_pattern)");
  else fail(`invoke(unknown) should name nearest matches: ${unknownText}`);
}

console.log(failures === 0 ? "\nPASS — S2 scale invariants hold" : `\nFAIL — ${failures} problem(s)`);
process.exit(failures === 0 ? 0 : 1);
