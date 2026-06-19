// Rao (parabolic-approximation) bell nozzle — TOP-style 80% bell — built as a
// watertight NURBS solid, with throat arcs + parabolic divergent contour, AND the
// gas-dynamics performance calcs derived from the as-built geometry.
const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";
const api = async (p, b) => {
  const r = await fetch(`${BASE}${p}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(b) });
  const t = await r.text(); let j; try { j = JSON.parse(t); } catch { j = t; }
  return { ok: r.ok, status: r.status, j, t };
};
const get = async (p) => { const r = await fetch(`${BASE}${p}`); const t = await r.text(); try { return JSON.parse(t); } catch { return t; } };
const del = (p) => fetch(`${BASE}${p}`, { method: "DELETE" });

// ── Rao geometry parameters ───────────────────────────────────────────────
const Rt = 1.0;                    // throat radius (kernel units; 1 u = 50 mm physical)
const eps = 25.0;                  // area expansion ratio A_e/A_t
const Re = Rt * Math.sqrt(eps);    // exit radius = 5.0
const Rc = 2.5;                    // chamber radius
const thetaN = 33 * Math.PI / 180; // initial parabola (turn) angle  [Rao chart, eps=25, 80% bell]
const thetaE = 9  * Math.PI / 180; // exit angle
const Ru = 1.5 * Rt;               // upstream throat arc radius (1.5 Rt)
const Rd = 0.382 * Rt;             // downstream throat arc radius (0.382 Rt, Rao)
const conv = 35 * Math.PI / 180;   // converging half-angle
const Ln = 0.8 * (Rt * (Math.sqrt(eps) - 1) / Math.tan(15 * Math.PI / 180)); // 80% bell length

// contour [z,r], throat at z=0 ----------------------------------------------
const C = [];
C.push([-3.4, Rc]);                                   // chamber inlet
const zTan = -Ru * Math.sin(conv), rTan = (Rt + Ru) - Ru * Math.cos(conv); // upstream-arc tangency
C.push([zTan - (Rc - rTan) / Math.tan(conv), Rc]);    // converging straight start (r=Rc)
for (let i = 0; i <= 4; i++) { const phi = conv * (1 - i / 4); C.push([-Ru * Math.sin(phi), (Rt + Ru) - Ru * Math.cos(phi)]); } // upstream arc -> throat (0,Rt)
for (let i = 1; i <= 4; i++) { const phi = thetaN * i / 4; C.push([Rd * Math.sin(phi), (Rt + Rd) - Rd * Math.cos(phi)]); }       // downstream arc throat -> N
const zN = Rd * Math.sin(thetaN), rN = (Rt + Rd) - Rd * Math.cos(thetaN);
const zE = zN + Ln, rE = Re;
const tn = Math.tan(thetaN), te = Math.tan(thetaE);
const zQ = ((rE - te * zE) - (rN - tn * zN)) / (tn - te), rQ = rN + tn * (zQ - zN); // bezier control = tangent intersection
for (let i = 1; i <= 12; i++) { const t = i / 12; const z = (1 - t) ** 2 * zN + 2 * (1 - t) * t * zQ + t ** 2 * zE; const r = (1 - t) ** 2 * rN + 2 * (1 - t) * t * rQ + t ** 2 * rE; C.push([z, r]); } // parabolic bell

const z0 = C[0][0];
const ring = (r, z, n = 64) => Array.from({ length: n }, (_, i) => { const a = i / n * 2 * Math.PI; return [r * Math.cos(a), r * Math.sin(a), z]; });
const sections = C.map(([z, r]) => ring(r, z - z0));

// ── gas dynamics (from the as-built ratios) ─────────────────────────────────
const g = 1.22, Rgas = 320, Tc = 3500, pc = 7.0e6, pa = 101325;   // chamber + propellant
const RtP = 0.05, AtP = Math.PI * RtP * RtP, AeP = eps * AtP;       // physical throat 50 mm
const areaMach = (M) => (1 / M) * ((2 / (g + 1)) * (1 + (g - 1) / 2 * M * M)) ** ((g + 1) / (2 * (g - 1)));
let lo = 1.0001, hi = 10; for (let i = 0; i < 80; i++) { const m = (lo + hi) / 2; if (areaMach(m) < eps) lo = m; else hi = m; } const Me = (lo + hi) / 2;
const pe_pc = (1 + (g - 1) / 2 * Me * Me) ** (-g / (g - 1));
const CFmom = Math.sqrt((2 * g * g / (g - 1)) * (2 / (g + 1)) ** ((g + 1) / (g - 1)) * (1 - pe_pc ** ((g - 1) / g)));
const CF = CFmom + (pe_pc - pa / pc) * eps;
const cstar = Math.sqrt(g * Rgas * Tc) / (g * Math.sqrt((2 / (g + 1)) ** ((g + 1) / (g - 1))));
const F = CF * pc * AtP, Isp = CF * cstar / 9.80665;

async function main() {
  await del("/api/agent/parts");
  const built = await api("/api/geometry/nurbs_loft", { sections, degree_u: 3, degree_v: 3, name: "Rao Bell Nozzle" });
  console.log("loft:", built.ok ? "ok" : `FAIL ${built.status} ${built.t.slice(0, 200)}`);
  const parts = await get("/api/agent/parts"); const sid = parts[parts.length - 1]?.id ?? 0;
  const truth = await get(`/api/agent/parts/${sid}/truth`);
  console.log("solid ground_truth:", (truth.summary ?? JSON.stringify(truth)).slice(0, 200));
  // Hollow it: open both planar caps, thin wall (NURBS shell — watertight per shell #22).
  const uuid = built.j?.object?.id;
  const pf = await api(`/api/agent/parts/${sid}/select-face`, { kind: "planar" });
  const caps = pf.j?.candidates ?? (pf.j?.face_id != null ? [pf.j.face_id] : []);
  if (uuid && caps.length >= 2) {
    const sh = await api("/api/geometry/shell", { object: uuid, thickness: 0.15, faces_to_remove: caps });
    console.log("shell:", sh.ok ? "ok" : `FAIL ${sh.status} ${sh.t.slice(0, 160)}`);
    const np = await get("/api/agent/parts"); const ns = np[np.length - 1]?.id ?? sid;
    const ht = await get(`/api/agent/parts/${ns}/truth`);
    console.log("HOLLOW ground_truth:", (ht.summary ?? JSON.stringify(ht)).slice(0, 200));
  } else console.log("shell skipped — caps:", caps);
  console.log("\n=== RAO CALCS ===");
  const o = { eps, Rt, Re, thetaN_deg: 33, thetaE_deg: 9, Ln, Me, pe_pc, CF, cstar, F_kN: F / 1000, Isp, AtP, AeP };
  console.log(JSON.stringify(o, (k, v) => typeof v === "number" ? +v.toFixed(4) : v, 2));
}
main().catch(e => { console.error("ERR", e.message); process.exit(1); });
