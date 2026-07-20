// Slices 3 + 4 — MCP scale architecture: workbenches + cad_program.
//
// Node-runnable WITHOUT a live backend (mocks the fetch layer). Exercises the
// compiled dist/ directly (build first: `npm run build`).
//
//   S3 WORKBENCH
//     (w1) status shape        — {active_bench, exposed_count, token_bill, benches}
//     (w2) switch changes set  — entering a bench exposes it, retires the previous
//     (w3) ≤35 in every bench  — live surface ceiling holds for all 5 + core_only
//     (w4) transition mechanics— apply(enable,disable) fires the right tool sets
//     (w5) invoke reaches      — a bench tool invokes while its bench is INACTIVE
//   S4 cad_program
//     (a) up-front validation  — 5 ops, invalid 3rd → typed report, ZERO executed
//     (b) stop-on-first-error  — 5 ops, 3rd fails → ledger [ok,ok,err], completed=2
//     (c) certificates present — ok ops carry the op's own soundness verdict
//     (d) recursion+destructive guards — meta ops + clear/delete refused at validation
//
// Run: node test/scale_s3s4.mjs   (exit 0 = pass, non-zero = fail)

import {
  buildTableWithControls,
  MINIMAL_SURFACE,
} from "../dist/surface.js";
import {
  Workbench,
  SWITCHABLE_BENCHES,
  LIVE_SURFACE_CEILING,
} from "../dist/workbench.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
const setEq = (a, b) => {
  const A = new Set(a), B = new Set(b);
  return A.size === B.size && [...A].every((x) => B.has(x));
};
/** Parse the first text content block of a tool result as JSON. */
const jsonOf = (res) => {
  const t = (res?.content ?? []).find((c) => c.type === "text");
  return t ? JSON.parse(t.text) : null;
};

// ─────────────────────────────────────────────────────────────────────────────
// S3 — WORKBENCHES
// ─────────────────────────────────────────────────────────────────────────────
console.log("(w1) WORKBENCH status shape");
{
  const { table, workbench } = buildTableWithControls();
  const res = await table.get("workbench").handler({ mode: "status" });
  const st = jsonOf(res);
  const hasKeys =
    st &&
    typeof st.active_bench === "string" &&
    typeof st.exposed_count === "number" &&
    typeof st.token_bill === "number" &&
    st.benches &&
    typeof st.benches === "object";
  if (hasKeys) pass(`status = {active_bench:${st.active_bench}, exposed_count:${st.exposed_count}, token_bill:${st.token_bill}, benches:{…}}`);
  else fail(`status shape wrong: ${JSON.stringify(st)}`);

  if (st.active_bench === "core_only") pass("fresh session reports active_bench=core_only");
  else fail(`fresh session active_bench=${st.active_bench}, expected core_only`);

  // benches map carries a count for every switchable bench.
  const missing = SWITCHABLE_BENCHES.filter((b) => typeof st.benches[b] !== "number");
  if (missing.length === 0) pass(`benches map counts all 5 benches: ${SWITCHABLE_BENCHES.map((b) => `${b}:${st.benches[b]}`).join(" ")}`);
  else fail(`benches map missing counts for: ${missing.join(", ")}`);

  if (st.exposed_count === MINIMAL_SURFACE.length)
    pass(`core_only exposed_count = ${st.exposed_count} = MINIMAL_SURFACE (${MINIMAL_SURFACE.length})`);
  else fail(`core_only exposed_count ${st.exposed_count} ≠ MINIMAL_SURFACE ${MINIMAL_SURFACE.length}`);
}

console.log("(w2) WORKBENCH switch changes the exposed set");
{
  const { workbench } = buildTableWithControls();
  const base = new Set(workbench.exposedNames());

  const sketchRes = workbench.enter("sketch");
  const afterSketch = new Set(workbench.exposedNames());
  const sketchTools = workbench.benchToolNames("sketch");
  const sketchExposed = sketchTools.every((n) => afterSketch.has(n));
  const coreStill = MINIMAL_SURFACE.every((n) => afterSketch.has(n));
  if (sketchExposed && coreStill && sketchRes.switched)
    pass(`enter('sketch') exposes ${sketchTools.length} sketch tools on top of core+meta (switched=true)`);
  else fail(`enter('sketch') did not expose sketch tools on top of core: switched=${sketchRes.switched}`);

  const asmRes = workbench.enter("assembly");
  const afterAsm = new Set(workbench.exposedNames());
  const sketchRetired = sketchTools.every((n) => !afterAsm.has(n));
  const asmTools = workbench.benchToolNames("assembly");
  const asmExposed = asmTools.every((n) => afterAsm.has(n));
  if (sketchRetired && asmExposed)
    pass("switching to 'assembly' retires the sketch bench and exposes assembly tools");
  else fail(`switch to assembly did not retire sketch / expose assembly (sketchRetired=${sketchRetired}, asmExposed=${asmExposed})`);

  const coreOnlyRes = workbench.enter("core_only");
  const afterCore = new Set(workbench.exposedNames());
  if (setEq([...afterCore], MINIMAL_SURFACE) && coreOnlyRes.active_bench === "core_only")
    pass("core_only retires the active bench back to exactly core+meta");
  else fail(`core_only did not return to minimal surface: ${[...afterCore].length} tools`);

  // Honesty caveat must be present on a real switch (spec §3.5).
  if (sketchRes.notice.includes("list_changed") && sketchRes.notice.toLowerCase().includes("invoke"))
    pass("switch notice carries the client-capability caveat + invoke-reaches-everything");
  else fail(`switch notice missing honesty caveat: ${sketchRes.notice}`);
}

console.log("(w3) WORKBENCH ≤35 live-surface ceiling in every bench");
{
  const { workbench } = buildTableWithControls();
  for (const bench of [...SWITCHABLE_BENCHES, "core_only"]) {
    const r = workbench.enter(bench);
    const n = r.exposed_count;
    if (n <= LIVE_SURFACE_CEILING && n >= MINIMAL_SURFACE.length)
      pass(`bench '${bench}': ${n} tools live (≤ ${LIVE_SURFACE_CEILING})`);
    else fail(`bench '${bench}': ${n} tools live — violates [${MINIMAL_SURFACE.length}, ${LIVE_SURFACE_CEILING}]`);
  }
}

console.log("(w4) WORKBENCH transition mechanics (apply enable/disable sets)");
{
  const { workbench } = buildTableWithControls();
  const calls = [];
  workbench.setApply((toEnable, toDisable) => calls.push({ toEnable, toDisable }));

  workbench.enter("drawing");
  const drawingTools = workbench.benchToolNames("drawing");
  if (calls.length === 1 && setEq(calls[0].toEnable, drawingTools) && calls[0].toDisable.length === 0)
    pass(`enter('drawing') → apply enables exactly the ${drawingTools.length} drawing tools, disables none`);
  else fail(`enter('drawing') apply wrong: ${JSON.stringify(calls[0])}`);

  workbench.enter("labels");
  const labelTools = workbench.benchToolNames("labels");
  const t = calls[1];
  if (t && setEq(t.toEnable, labelTools) && setEq(t.toDisable, drawingTools))
    pass("switch drawing→labels → apply enables label tools AND disables drawing tools");
  else fail(`drawing→labels apply wrong: ${JSON.stringify(t)}`);
}

console.log("(w5) WORKBENCH: invoke reaches a bench tool while its bench is INACTIVE");
{
  const { table, workbench } = buildTableWithControls();
  // Make 'sketch' active so the 'labels' bench is inactive (label_list NOT exposed).
  workbench.enter("sketch");
  const exposed = new Set(workbench.exposedNames());
  const labelsInactive = !exposed.has("label_list");

  const realFetch = globalThis.fetch;
  let called = 0;
  globalThis.fetch = async (url, init = {}) => {
    called += 1;
    const payload = String(url).includes("/labels")
      ? [{ name: "throat", kind: "face" }]
      : {};
    return { ok: true, status: 200, text: async () => JSON.stringify(payload) };
  };
  try {
    const res = await table.get("invoke").handler({ name: "label_list", args: { part_id: 1 } });
    const text = (res?.content ?? []).map((c) => c.text ?? "").join("");
    if (labelsInactive && !res.isError && text.includes("throat") && called > 0)
      pass("invoke('label_list') dispatched and returned data while the labels bench was INACTIVE (capability never gated)");
    else fail(`invoke did not reach inactive-bench tool: inactive=${labelsInactive} isError=${res.isError} called=${called} text=${text.slice(0, 80)}`);
  } finally {
    globalThis.fetch = realFetch;
  }
}

console.log("(w6) WORKBENCH: full surface mode → switching is a no-op with an honest notice");
{
  const prev = process.env.ROSHERA_MCP_SURFACE;
  process.env.ROSHERA_MCP_SURFACE = "full";
  try {
    const { workbench } = buildTableWithControls();
    const before = new Set(workbench.exposedNames());
    const r = workbench.enter("sketch");
    const after = new Set(workbench.exposedNames());
    if (!r.switched && setEq([...before], [...after]) && r.active_bench === "full")
      pass("full mode: workbench('sketch') did not change exposure (switched=false, active_bench=full)");
    else fail(`full mode switch was not a no-op: switched=${r.switched} active=${r.active_bench}`);
    if (r.notice.toLowerCase().includes("full") && r.notice.toLowerCase().includes("no-op"))
      pass("full mode notice honestly states the no-op");
    else fail(`full mode notice unclear: ${r.notice}`);
  } finally {
    if (prev === undefined) delete process.env.ROSHERA_MCP_SURFACE;
    else process.env.ROSHERA_MCP_SURFACE = prev;
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// S4 — cad_program
// ─────────────────────────────────────────────────────────────────────────────

// A create_box-shaped mock backend. `failBoxAt` (1-based) makes the Nth box POST
// return 400. Records every box POST so we can assert ops after a stop never ran.
function installBoxMock({ failBoxAt = 0 } = {}) {
  const real = globalThis.fetch;
  const state = { boxPosts: 0 };
  globalThis.fetch = async (url, init = {}) => {
    const method = init.method ?? "GET";
    const u = String(url);
    if (method === "POST" && u.includes("/api/geometry/box")) {
      state.boxPosts += 1;
      if (failBoxAt && state.boxPosts === failBoxAt) {
        return { ok: false, status: 400, text: async () => "kernel refused: degenerate box" };
      }
      // Body carries the cheap embedded verdict → api() stashes it → okp reuses it.
      return {
        ok: true,
        status: 200,
        text: async () =>
          JSON.stringify({
            object: { id: `uuid-${state.boxPosts}` },
            valid: true,
            watertight: true,
            face_count: 6,
            volume: 1000,
            dims: [10, 10, 5],
          }),
      };
    }
    if (method === "GET" && u.endsWith("/api/agent/parts")) {
      return { ok: true, status: 200, text: async () => JSON.stringify([{ id: 1 }]) };
    }
    if (method === "GET" && u.includes("/api/agent/parts/")) {
      return {
        ok: true,
        status: 200,
        text: async () =>
          JSON.stringify({ location: { center_world: [0, 0, 2.5], dimensions_world: [10, 10, 5] } }),
      };
    }
    return { ok: true, status: 200, text: async () => "{}" };
  };
  return { state, restore: () => (globalThis.fetch = real) };
}

const boxOp = (w = 10) => ({ tool: "create_box", args: { width: w, depth: 10, height: 5 } });

console.log("(a) cad_program: invalid 3rd op → typed validation report, ZERO executed");
{
  const { table } = buildTableWithControls();
  const { state, restore } = installBoxMock();
  try {
    const ops = [
      boxOp(1),
      boxOp(2),
      { tool: "create_box", args: { width: 5, depth: 5 } }, // missing required `height`
      boxOp(4),
      boxOp(5),
    ];
    const res = await table.get("cad_program").handler({ name: "p", ops });
    const rep = jsonOf(res);
    if (res.isError && rep.stage === "validation" && rep.executed === 0)
      pass("invalid batch refused at validation stage, executed=0");
    else fail(`expected validation-stage refusal: ${JSON.stringify(rep)}`);

    if (rep.errors?.length === 1 && rep.errors[0].index === 2)
      pass(`per-op report names exactly op index 2: "${rep.errors[0].reason.slice(0, 60)}…"`);
    else fail(`validation errors wrong: ${JSON.stringify(rep.errors)}`);

    if (state.boxPosts === 0) pass("ZERO backend calls made — nothing executed before the whole batch validated");
    else fail(`expected 0 box POSTs, got ${state.boxPosts}`);
  } finally {
    restore();
  }
}

console.log("(b) cad_program: 3rd op fails at execution → ledger [ok,ok,err], completed=2, 4-5 not attempted");
{
  const { table } = buildTableWithControls();
  const { state, restore } = installBoxMock({ failBoxAt: 3 });
  try {
    const ops = [boxOp(1), boxOp(2), boxOp(3), boxOp(4), boxOp(5)];
    const res = await table.get("cad_program").handler({ ops });
    const led = jsonOf(res);
    if (led.completed === 2 && led.total === 5 && led.stopped_at === 2)
      pass("ledger: completed=2, total=5, stopped_at=2");
    else fail(`ledger counts wrong: ${JSON.stringify({ completed: led.completed, total: led.total, stopped_at: led.stopped_at })}`);

    const shape = led.ops.map((o) => (o.ok ? "ok" : "err"));
    if (led.ops.length === 3 && setEq(shape, ["ok", "err"]) && shape[0] === "ok" && shape[1] === "ok" && shape[2] === "err")
      pass("ledger.ops = [ok, ok, err] (only attempted ops recorded)");
    else fail(`ledger.ops shape wrong: ${JSON.stringify(shape)}`);

    if (led.ops[2].error && led.ops[2].error.includes("400"))
      pass(`stopped op carries the typed backend error: "${led.ops[2].error.slice(0, 50)}…"`);
    else fail(`stopped op missing typed error: ${JSON.stringify(led.ops[2])}`);

    // The mock recorded exactly 3 box POSTs — ops 4 & 5 were never attempted.
    if (state.boxPosts === 3) pass("backend saw exactly 3 box POSTs — ops 4 & 5 never attempted (state matches ledger)");
    else fail(`expected 3 box POSTs, got ${state.boxPosts}`);

    if (led.ok === false && led.note.toLowerCase().includes("no rollback"))
      pass("ledger is honest: ok=false, note states no rollback / state matches the ledger");
    else fail(`ledger honesty note wrong: ${led.note}`);
  } finally {
    restore();
  }
}

console.log("(c) cad_program: ok ops carry the op's own soundness certificate");
{
  const { table } = buildTableWithControls();
  const { restore } = installBoxMock();
  try {
    const res = await table.get("cad_program").handler({ ops: [boxOp(1), boxOp(2)] });
    const led = jsonOf(res);
    const allCertified = led.ops.length === 2 && led.ops.every((o) => o.ok && typeof o.certificate === "string" && o.certificate.includes("SOUND"));
    if (led.ok === true && allCertified)
      pass(`both ok ops carry a SOUND certificate: "${led.ops[0].certificate.slice(0, 48)}…"`);
    else fail(`certificates missing/incomplete: ${JSON.stringify(led.ops)}`);
  } finally {
    restore();
  }
}

console.log("(d) cad_program: recursion + destructive guards (refused at validation)");
{
  const { table } = buildTableWithControls();

  // recursion: meta/composition tools cannot be ops.
  const metaRes = await table.get("cad_program").handler({
    ops: [
      { tool: "invoke", args: {} },
      { tool: "workbench", args: { mode: "status" } },
      { tool: "cad_program", args: { ops: [] } },
    ],
  });
  const metaRep = jsonOf(metaRes);
  const flaggedMeta = new Set((metaRep.errors ?? []).map((e) => e.tool));
  if (metaRes.isError && metaRep.executed === 0 && ["invoke", "workbench", "cad_program"].every((t) => flaggedMeta.has(t)))
    pass("meta ops (invoke/workbench/cad_program) refused as program ops, executed=0");
  else fail(`meta guard failed: ${JSON.stringify(metaRep.errors)}`);

  // destructive blocked without the flag.
  const destRes = await table.get("cad_program").handler({
    ops: [boxOp(1), { tool: "delete_part", args: { part_id: 1 } }],
  });
  const destRep = jsonOf(destRes);
  const delErr = (destRep.errors ?? []).find((e) => e.tool === "delete_part");
  if (destRes.isError && destRep.executed === 0 && delErr && delErr.reason.includes("allow_destructive"))
    pass("delete_part refused without allow_destructive (footgun guard), executed=0");
  else fail(`destructive guard failed: ${JSON.stringify(destRep.errors)}`);

  // allow_destructive clears the destructive guard (proven at validation: a
  // DIFFERENT invalid op stops the batch, and delete_part is NOT among the errors).
  const allowRes = await table.get("cad_program").handler({
    allow_destructive: true,
    ops: [
      { tool: "delete_part", args: { part_id: 1 } },
      { tool: "create_box", args: { depth: 1, height: 1 } }, // invalid: missing width
    ],
  });
  const allowRep = jsonOf(allowRes);
  const errTools = new Set((allowRep.errors ?? []).map((e) => e.tool));
  if (allowRes.isError && errTools.has("create_box") && !errTools.has("delete_part"))
    pass("allow_destructive:true clears the delete_part guard (only the genuinely-invalid op is flagged)");
  else fail(`allow_destructive did not clear the guard: ${JSON.stringify(allowRep.errors)}`);
}

console.log(failures === 0 ? "\nPASS — S3/S4 invariants hold" : `\nFAIL — ${failures} problem(s)`);
process.exit(failures === 0 ? 0 : 1);
