// Rao bell nozzle as a REVOLVED closed wall-profile (NURBS meridian) — the right
// way to get a SMOOTH thin wall: revolve a closed [r,z] cross-section (outer Rao
// contour out, inner contour back), so inner + outer follow the contour exactly.
// No surface-offset approximation → no discrete bump on the inner wall.
const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";
const api = async (p, b) => { const r = await fetch(`${BASE}${p}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(b) }); const t = await r.text(); let j; try { j = JSON.parse(t); } catch { j = t; } return { ok: r.ok, status: r.status, j, t }; };
const get = async (p) => { const r = await fetch(`${BASE}${p}`); const t = await r.text(); try { return JSON.parse(t); } catch { return t; } };
const del = (p) => fetch(`${BASE}${p}`, { method: "DELETE" });

// ── Rao geometry (meridian as {r,z}, throat at z=0) ─────────────────────────
const Rt = 1.0, eps = 25, Re = Rt * Math.sqrt(eps), Rc = 2.5;
const thetaN = 33 * Math.PI / 180, thetaE = 9 * Math.PI / 180;
const Ru = 1.5 * Rt, Rd = 0.382 * Rt, conv = 35 * Math.PI / 180;
const Ln = 0.8 * (Rt * (Math.sqrt(eps) - 1) / Math.tan(15 * Math.PI / 180));
const t = 0.15; // wall thickness

const O = []; // outer meridian {r,z}
O.push({ r: Rc, z: -3.4 });
const zTan = -Ru * Math.sin(conv), rTan = (Rt + Ru) - Ru * Math.cos(conv);
O.push({ r: Rc, z: zTan - (Rc - rTan) / Math.tan(conv) });
for (let i = 0; i <= 6; i++) { const phi = conv * (1 - i / 6); O.push({ r: (Rt + Ru) - Ru * Math.cos(phi), z: -Ru * Math.sin(phi) }); }   // upstream arc → throat
for (let i = 1; i <= 6; i++) { const phi = thetaN * i / 6; O.push({ r: (Rt + Rd) - Rd * Math.cos(phi), z: Rd * Math.sin(phi) }); }            // downstream arc → N
const zN = Rd * Math.sin(thetaN), rN = (Rt + Rd) - Rd * Math.cos(thetaN), zE = zN + Ln, rE = Re;
const tn = Math.tan(thetaN), te = Math.tan(thetaE);
const zQ = ((rE - te * zE) - (rN - tn * zN)) / (tn - te), rQ = rN + tn * (zQ - zN);
for (let i = 1; i <= 20; i++) { const u = i / 20; const z = (1 - u) ** 2 * zN + 2 * (1 - u) * u * zQ + u ** 2 * zE; const r = (1 - u) ** 2 * rN + 2 * (1 - u) * u * rQ + u ** 2 * rE; O.push({ r, z }); } // parabolic bell

// inner meridian: perpendicular inward offset by t (constant wall thickness)
const I = O.map((p, i) => {
  const a = O[Math.max(0, i - 1)], b = O[Math.min(O.length - 1, i + 1)];
  let tr = b.r - a.r, tz = b.z - a.z; const L = Math.hypot(tr, tz) || 1; tr /= L; tz /= L;
  let nr = tz, nz = -tr;                 // outward normal (toward +r)
  if (nr < 0) { nr = -nr; nz = -nz; }    // ensure it points outward (+r)
  return { r: p.r - t * nr, z: p.z - t * nz };
});

// closed wall cross-section: outer chamber→exit, then inner exit→chamber
const profile = [...O.map((p) => [p.r, p.z]), ...I.slice().reverse().map((p) => [p.r, p.z])];

// Print the wall-profile as a compact rounded [r,z] array — to feed to the MCP `revolve` tool.
const round = (x) => Math.round(x * 1000) / 1000;
console.log(JSON.stringify(profile.map((p) => [round(p[0]), round(p[1])])));
console.error(`profile pts: ${profile.length}, wall t=${t}`);
