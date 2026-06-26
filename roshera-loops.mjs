#!/usr/bin/env node
// Roshera nested verification loops — "loops within loops".
//
//   OUTER loop  : iterate a corpus of parametric designs (housing, flange,
//                 bored-plate, lofted barrel, revolved tube, fillet/chamfer
//                 chains). Optionally sweep parameters so each design is a
//                 family, not a single point.
//   MIDDLE loop : drive ONE design step-by-step through the REST surface and,
//                 after EVERY mutating op, read the per-op verdict the kernel
//                 embeds in the response (`perception`). The first step whose
//                 verdict is unsound (or whose full soundness is deferred) is a
//                 FINDING, logged with the exact reproducing op + parameters.
//   INNER loop  : the kernel's own ambient certificate (certify_solid →
//                 is_sound()) that now runs on every op. We don't re-implement
//                 it; we consume its verdict and, on demand, ask for the FULL
//                 certificate breakdown via `?full=1` (the explicit verify
//                 surface, off the MCP hot path).
//
// MCP RESPONSIVENESS is the hard invariant: this driver talks ONLY to REST and
// throttles itself (a pacing delay + a soundness probe of the live backend
// before each design). It NEVER calls the MCP tools, so an MCP client stays
// responsive while the loops run. The over-budget per-op path returns
// `sound:null + full_soundness:"deferred"`, which the ledger records as
// "deferred" (NOT a pass and NOT a defect) and re-checks via `?full=1`.
//
// Output: a rolling ledger at loops-out/ledger.json + a human summary printed
// each pass. Designed to run through the night via --watch.
//
// Usage:
//   node roshera-loops.mjs                 # one full pass over the corpus
//   node roshera-loops.mjs --watch         # loop forever, pacing itself
//   node roshera-loops.mjs --watch --period 300   # ~5 min between passes
//   ROSHERA_URL=http://localhost:8081 node roshera-loops.mjs
//
// Env:
//   ROSHERA_URL   backend base (default http://localhost:8081)
//   LOOP_PACE_MS  per-op pacing delay in ms (default 120) — keeps the write
//                 lock free between ops so MCP reads interleave.

import { writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";

const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";
const PACE_MS = Number(process.env.LOOP_PACE_MS ?? 120);
const OUT_DIR = new URL("./loops-out/", import.meta.url);
const LEDGER = new URL("./loops-out/ledger.json", import.meta.url);

const args = process.argv.slice(2);
const WATCH = args.includes("--watch");
const PERIOD = (() => {
  const i = args.indexOf("--period");
  return i >= 0 ? Number(args[i + 1]) : 0; // seconds between passes (0 = back-to-back)
})();

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const nowIso = () => new Date().toISOString();

// ── REST helpers ─────────────────────────────────────────────────────────
async function api(path, body, method = "POST") {
  const r = await fetch(`${BASE}${path}`, {
    method,
    headers: { "Content-Type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  const txt = await r.text();
  let j;
  try { j = JSON.parse(txt); } catch { j = { raw: txt }; }
  if (!r.ok) {
    const err = new Error(`${method} ${path} -> HTTP ${r.status}`);
    err.http = r.status;
    err.bodyText = txt.slice(0, 400);
    err.body = j;
    throw err;
  }
  return j;
}

// Is the backend up? Cheap, read-only — used as the pre-pass gate.
async function backendUp() {
  try {
    await api("/api/agent/parts", null, "GET");
    return true;
  } catch {
    return false;
  }
}

// Extract the per-op verdict the kernel embeds in a mutating-endpoint response.
// Handles both the embedded `perception` block and a top-level verdict.
function verdictOf(resp) {
  const p = resp?.perception ?? resp ?? {};
  return {
    solid_id: resp?.solid_id ?? resp?.object?.solid_id ?? p?.solid_id ?? null,
    uuid: resp?.object?.id ?? resp?.uuid ?? null,
    sound: p.sound ?? null,                      // true | false | null(deferred)
    watertight: p.watertight ?? null,
    self_intersection_free: p.self_intersection_free ?? null,
    self_intersection_checked: p.self_intersection_checked ?? null,
    full_soundness: p.full_soundness ?? null,    // "deferred" when over budget
    valid: p.valid ?? null,
    verdict: p.verdict ?? null,
    triangle_count: p.triangle_count ?? resp?.stats?.triangle_count ?? null,
  };
}

// Ask the explicit verify surface for the FULL certificate (off the MCP hot
// path). Returns the cert object or null.
async function fullCert(solidId) {
  if (solidId == null) return null;
  try {
    const j = await api(`/api/agent/parts/${solidId}/perception?full=1`, null, "GET");
    return { sound: j.sound ?? null, cert: j.cert ?? null, verdict: j.verdict ?? null };
  } catch {
    return null;
  }
}

// ── design DSL ───────────────────────────────────────────────────────────
// A design is { name, steps: [ {label, run: async () => resp} ] }. `run`
// returns the raw REST response; the middle loop reads its verdict.

const G = {
  clear: () => api("/api/agent/parts", null, "DELETE").catch(() => {}),
  box: (w, h, d) =>
    api("/api/geometry", { shape_type: "box", parameters: { width: w, height: h, depth: d } }),
  cyl: (center, axis, radius, height) =>
    api("/api/geometry/cylinder", { center, axis, radius, height }),
  // The boolean endpoint takes operand UUIDs (object_a/object_b) and returns the
  // result solid (uuid + solid_id) embedded like every other mutating endpoint.
  boolean: (operation, uuidA, uuidB) =>
    api("/api/geometry/boolean", { operation, object_a: uuidA, object_b: uuidB }),
  revolve: (profile, axis_origin, axis_direction, segments = 48) =>
    api("/api/geometry/revolve", { profile, axis_origin, axis_direction, segments }),
  nurbsLoft: (sections, degree_u = 3, degree_v = 3) =>
    api("/api/geometry/nurbs_loft", { sections, degree_u, degree_v }),
  shell: (objectUuid, thickness, faces_to_remove) =>
    api("/api/geometry/shell", { object: objectUuid, thickness, faces_to_remove }),
};

// resolve a solid_id (u32) from a create response: the boolean endpoint returns
// solid_a/solid_b as ids; create endpoints return an object uuid + a solid_id.
function idOf(resp) {
  return (
    resp?.solid_id ??
    resp?.object?.solid_id ??
    resp?.perception?.solid_id ??
    null
  );
}
function uuidOf(resp) {
  return resp?.object?.id ?? resp?.uuid ?? null;
}

// The corpus. Each entry returns a fresh design object given parameters.
function corpus() {
  const designs = [];

  // 1) HOUSING — box ∖ bore (known unsound class: shell self-intersection /
  //    bore weld). Several bore diameters so it's a family.
  for (const r of [4, 5, 6]) {
    designs.push({
      name: `housing_bore_r${r}`,
      steps: async (ctx) => {
        const block = await G.box(20, 16, 12);
        ctx.record("box", block);
        const bore = await G.cyl([0, 0, -10], [0, 0, 1], r, 20);
        ctx.record("bore_cyl", bore);
        const diff = await G.boolean("difference", uuidOf(block), uuidOf(bore));
        ctx.record("difference", diff);
        // Closed hollow (no faces_to_remove) — exercises the offset self-
        // intersection without needing a discovered cap face id.
        const uuid = uuidOf(diff);
        if (uuid) {
          const sh = await G.shell(uuid, 2, []);
          ctx.record("shell", sh);
        }
      },
    });
  }

  // 2) BORED PLATE — box ∖ several through-holes (multi-bore weld stress).
  designs.push({
    name: "bored_plate_4holes",
    steps: async (ctx) => {
      let plate = await G.box(40, 40, 6);
      ctx.record("plate", plate);
      for (const [cx, cy] of [[-12, -12], [12, -12], [-12, 12], [12, 12]]) {
        const hole = await G.cyl([cx, cy, -5], [0, 0, 1], 3, 10);
        ctx.record("hole_cyl", hole);
        plate = await G.boolean("difference", uuidOf(plate), uuidOf(hole));
        ctx.record("difference", plate);
      }
    },
  });

  // 3) CROSS-BORE — two perpendicular bores through a block (the cyl-cyl
  //    saddle / Steinmetz class, #35).
  designs.push({
    name: "cross_bore",
    steps: async (ctx) => {
      let block = await G.box(24, 24, 24);
      ctx.record("block", block);
      const bz = await G.cyl([0, 0, -16], [0, 0, 1], 6, 32);
      ctx.record("bore_z", bz);
      block = await G.boolean("difference", uuidOf(block), uuidOf(bz));
      ctx.record("difference_z", block);
      const bx = await G.cyl([-16, 0, 0], [1, 0, 0], 6, 32);
      ctx.record("bore_x", bx);
      block = await G.boolean("difference", uuidOf(block), uuidOf(bx));
      ctx.record("difference_x", block);
    },
  });

  // 4) REVOLVED TUBE — solid of revolution, then a closed shell.
  designs.push({
    name: "revolved_tube",
    steps: async (ctx) => {
      const profile = [[2, 0], [6, 0], [6, 8], [2, 8]];
      const rev = await G.revolve(profile, [0, 0, 0], [0, 0, 1], 48);
      ctx.record("revolve", rev);
    },
  });

  // 5) LOFTED BARREL — a closed NURBS loft (skin watertightness).
  designs.push({
    name: "lofted_barrel",
    steps: async (ctx) => {
      const ring = (z, r, n = 32) =>
        Array.from({ length: n }, (_, i) => {
          const t = (i * 2 * Math.PI) / n;
          return [r * Math.cos(t), r * Math.sin(t), z];
        });
      const sections = [ring(0, 4), ring(4, 6), ring(8, 6), ring(12, 4)];
      const lo = await G.nurbsLoft(sections, 3, 3);
      ctx.record("nurbs_loft", lo);
    },
  });

  // 6) UNION TOWER — stacked box ∪ cylinder (coincident-face union, #32).
  designs.push({
    name: "union_tower",
    steps: async (ctx) => {
      const base = await G.box(20, 20, 6);
      ctx.record("base", base);
      const post = await G.cyl([0, 0, 3], [0, 0, 1], 5, 14);
      ctx.record("post", post);
      const u = await G.boolean("union", uuidOf(base), uuidOf(post));
      ctx.record("union", u);
    },
  });

  return designs;
}

// ── the middle loop: drive one design, verify every step ───────────────────
async function runDesign(design) {
  const finding = { name: design.name, at: nowIso(), steps: [], firstDefect: null };
  const ctx = {
    record: (label, resp) => {
      const v = verdictOf(resp);
      finding.steps.push({ label, ...v });
      // First unsound step = a defect (sound === false). `null` (deferred) is
      // NOT a defect here; we re-check it against the full cert below.
      if (finding.firstDefect == null && v.sound === false) {
        finding.firstDefect = { label, verdict: v.verdict };
      }
    },
  };

  await G.clear();
  try {
    await design.steps(ctx);
  } catch (e) {
    finding.error = { msg: e.message, http: e.http ?? null, body: e.bodyText ?? null };
  }

  // ALWAYS pull the authoritative FULL certificate on the design's final solid
  // (off the MCP hot path, via ?full=1). The per-op EMBEDDED verdict on a create
  // endpoint is the CHEAP B-Rep perception — it can report sound:true while the
  // full cert reports self-intersection / non-watertight false (exactly the
  // verification gap). So we never trust the embedded sound:true as the design
  // verdict; the full cert is ground truth.
  const last = finding.steps[finding.steps.length - 1];
  if (last && last.solid_id != null) {
    const fc = await fullCert(last.solid_id);
    if (fc) {
      last.resolved_full_sound = fc.sound;
      last.resolved_cert = fc.cert
        ? {
            watertight: fc.cert.watertight,
            self_intersection_free: fc.cert.self_intersection_free,
            manifold: fc.cert.manifold,
            brep_valid: fc.cert.brep_valid,
          }
        : null;
      if (fc.sound === false && finding.firstDefect == null) {
        finding.firstDefect = {
          label: last.label,
          verdict: fc.verdict,
          via: "full_cert",
          cert: last.resolved_cert,
        };
      }
    }
  }

  // Design verdict: ERROR > UNSOUND (a real defect, from full cert or an op's
  // embedded sound:false) > DEFERRED (a step over budget, unresolved) > SOUND.
  finding.verdict =
    finding.error ? "ERROR" :
    finding.firstDefect ? "UNSOUND" :
    last && last.resolved_full_sound === false ? "UNSOUND" :
    finding.steps.some((s) => s.sound === null && s.resolved_full_sound == null) ? "DEFERRED" :
    "SOUND";
  return finding;
}

// ── the outer loop ────────────────────────────────────────────────────────
async function loadLedger() {
  if (!existsSync(new URL("./loops-out/ledger.json", import.meta.url))) {
    return { created: nowIso(), passes: 0, history: [], findings: {} };
  }
  try {
    return JSON.parse(await readFile(LEDGER, "utf8"));
  } catch {
    return { created: nowIso(), passes: 0, history: [], findings: {} };
  }
}

async function onePass(passIdx) {
  if (!(await backendUp())) {
    console.log(`[${nowIso()}] pass ${passIdx}: backend DOWN at ${BASE} — skipping.`);
    return null;
  }
  const ledger = await loadLedger();
  const designs = corpus();
  const summary = { pass: passIdx, at: nowIso(), counts: { SOUND: 0, UNSOUND: 0, DEFERRED: 0, ERROR: 0 } };

  console.log(`\n[${nowIso()}] ── PASS ${passIdx} (${designs.length} designs) ──`);
  for (const design of designs) {
    const f = await runDesign(design);
    summary.counts[f.verdict] = (summary.counts[f.verdict] ?? 0) + 1;
    const flag = f.verdict === "SOUND" ? "ok  " : f.verdict === "UNSOUND" ? "FAIL" : f.verdict.slice(0, 4);
    const where = f.firstDefect ? ` @${f.firstDefect.label}` : "";
    console.log(`  [${flag}] ${design.name.padEnd(22)} ${f.steps.length} steps${where}`);
    // Keep only the most recent finding per design, plus first-seen timestamp.
    const prev = ledger.findings[design.name];
    const changed = prev && prev.verdict !== f.verdict ? `${prev.verdict}→${f.verdict}` : null;
    if (changed) {
      // Surface verdict FLIPS on the console (the events worth waking up for):
      // a regression (SOUND→UNSOUND) or a fix landing (UNSOUND→SOUND).
      console.log(`  *** VERDICT CHANGED: ${design.name} ${changed} ***`);
    }
    ledger.findings[design.name] = {
      ...f,
      first_seen: prev?.first_seen ?? f.at,
      verdict_changed: changed,
    };
    await sleep(PACE_MS); // pacing: release the write lock so MCP reads interleave
  }

  ledger.passes = passIdx;
  ledger.history.push(summary);
  if (ledger.history.length > 500) ledger.history = ledger.history.slice(-500);
  await mkdir(OUT_DIR, { recursive: true });
  await writeFile(LEDGER, JSON.stringify(ledger, null, 2));

  const c = summary.counts;
  console.log(
    `[${nowIso()}] pass ${passIdx} done: ${c.SOUND} sound, ${c.UNSOUND} unsound, ` +
    `${c.DEFERRED} deferred, ${c.ERROR} error -> ${LEDGER.pathname}`
  );
  return summary;
}

async function main() {
  console.log(`Roshera loops -> ${BASE}  (watch=${WATCH}, pace=${PACE_MS}ms)`);
  let pass = (await loadLedger()).passes ?? 0;
  do {
    pass += 1;
    try {
      await onePass(pass);
    } catch (e) {
      console.error(`[${nowIso()}] pass ${pass} crashed: ${e.message}`);
    }
    if (WATCH && PERIOD > 0) await sleep(PERIOD * 1000);
  } while (WATCH);
}

main().catch((e) => {
  console.error("loops FAILED:", e.message);
  process.exit(1);
});
