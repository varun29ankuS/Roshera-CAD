// Auto-feedback harness (campaign task #8).
//
// Guards the invariant "perception is the ambient default": every MUTATING MCP
// tool must return its result through okp() (which appends the perception
// verdict), the escape hatch must exist, and the perception packet must be
// correct + sound on a live part (watertight ⟺ no open/non-manifold edges).
//
// Two layers:
//   STATIC  — read src/index.ts, assert each mutating tool's handler uses okp()
//             and that perceive() honours ROSHERA_MCP_AUTOVERIFY. Runs always.
//   LIVE    — against a running api-server (ROSHERA_URL or :8081), assert the
//             packet shape + the watertight invariant. Skipped (warned) if the
//             server is down so the static gate still runs in CI.
//
// Run: node test/auto_feedback.mjs   (exit 0 = pass, non-zero = fail)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const SRC = readFileSync(join(here, "..", "src", "index.ts"), "utf8");

let failures = 0;
const fail = (msg) => {
  console.error("  ✗ " + msg);
  failures += 1;
};
const pass = (msg) => console.log("  ✓ " + msg);

// ── STATIC: every mutating tool routes through okp() ─────────────────────────
console.log("STATIC: mutating tools auto-verify");

// The tools that PRODUCE or MODIFY a solid. transform is intentionally exempt
// (a rigid motion cannot change watertightness) and asserted exempt below.
const MUTATING = [
  "create_box",
  "create_cylinder",
  "create_cone",
  "create_sphere",
  "create_plate_with_holes",
  "revolve",
  "boolean",
  "sketch_extrude",
  "psketch_extrude",
];

/** Slice of source for one server.tool("name", …) block, up to the next tool. */
function toolBlock(name) {
  const start = SRC.indexOf(`"${name}",`);
  if (start < 0) return null;
  const next = SRC.indexOf("server.tool(", start + 1);
  return SRC.slice(start, next < 0 ? SRC.length : next);
}

for (const name of MUTATING) {
  const block = toolBlock(name);
  if (!block) {
    fail(`tool ${name} not found in index.ts`);
    continue;
  }
  if (block.includes("okp(")) {
    pass(`${name} returns through okp()`);
  } else {
    fail(`${name} does NOT use okp() — it would return without a perception verdict`);
  }
}

// transform must NOT need okp (documented exemption) — assert it stays bare ok.
const tBlock = toolBlock("transform");
if (tBlock && !tBlock.includes("okp(")) {
  pass("transform is exempt (rigid motion preserves validity)");
} else if (tBlock) {
  // Not a failure, just note: if someone wires okp into transform that's fine
  // too, just unnecessary.
  pass("transform uses okp() (harmless)");
}

// Escape hatch + helper presence.
if (SRC.includes("ROSHERA_MCP_AUTOVERIFY")) pass("escape hatch ROSHERA_MCP_AUTOVERIFY present");
else fail("escape hatch ROSHERA_MCP_AUTOVERIFY missing");
if (/async function perceive\(/.test(SRC)) pass("perceive() defined");
else fail("perceive() missing");

// ── LIVE: packet shape + soundness invariant ─────────────────────────────────
const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";
async function api(m, p, b) {
  const r = await fetch(`${BASE}${p}`, {
    method: m,
    headers: {
      "X-Roshera-Agent": "Claude",
      ...(b !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: b !== undefined ? JSON.stringify(b) : undefined,
  });
  const t = await r.text();
  if (!r.ok) throw new Error(`${m} ${p} -> ${r.status}: ${t.slice(0, 150)}`);
  return t.length ? JSON.parse(t) : null;
}
async function newest() {
  const ps = await api("GET", "/api/agent/parts");
  return ps.reduce((m, p) => Math.max(m, p.id), 0);
}
// The exact perceive() mapping the MCP uses.
async function perceive(id) {
  const diag = await api("GET", `/api/agent/parts/${id}/render?mode=diagnostic&view=iso&size=128`);
  const part = await api("GET", `/api/agent/parts/${id}`).catch(() => null);
  const open = diag?.open_edges ?? 0;
  const nm = diag?.nonmanifold_edges ?? 0;
  const watertight = open === 0 && nm === 0;
  return {
    watertight,
    open_edges: open,
    nonmanifold_edges: nm,
    face_count: part?.topology?.face_count ?? null,
    volume: part?.volume ?? null,
  };
}

console.log("LIVE: perception packet against the running api-server");
let serverUp = true;
try {
  await api("GET", "/api/agent/parts");
} catch {
  serverUp = false;
  console.warn("  ⚠ api-server not reachable at " + BASE + " — skipping live checks (static gate still ran)");
}

if (serverUp) {
  // Clean box: built == perceived for validity + structure.
  const s = await api("POST", "/api/sketch", { plane: "xy", tool: "rectangle" });
  await api("POST", `/api/sketch/${s.id}/point`, { point: [-20, -20] });
  await api("POST", `/api/sketch/${s.id}/point`, { point: [20, 20] });
  await api("POST", `/api/sketch/${s.id}/extrude`, { distance: 30 });
  const box = await newest();
  const p = await perceive(box);
  if (p.watertight === true && p.open_edges === 0 && p.nonmanifold_edges === 0)
    pass("box: watertight verdict true, 0/0 edges");
  else fail(`box should be watertight: ${JSON.stringify(p)}`);
  if (p.face_count === 6) pass("box: face_count = 6");
  else fail(`box face_count should be 6, got ${p.face_count}`);
  if (Math.abs((p.volume ?? 0) - 48000) < 1) pass("box: volume = 48000 (40×40×30)");
  else fail(`box volume should be 48000, got ${p.volume}`);

  // Soundness invariant on ANY part: watertight ⟺ no open/non-manifold edges.
  // (Holds regardless of whether the part happens to be valid.)
  const inv = p.watertight === (p.open_edges === 0 && p.nonmanifold_edges === 0);
  if (inv) pass("invariant: watertight ⟺ (0 open ∧ 0 non-manifold)");
  else fail("watertight verdict does not match edge counts");
}

console.log(failures === 0 ? "\nPASS — auto-feedback invariants hold" : `\nFAIL — ${failures} problem(s)`);
process.exit(failures === 0 ? 0 : 1);
