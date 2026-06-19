// Rocket nozzle: a REFINED converging–diverging bell (Rao-ish, ~27:1 expansion)
// as a watertight NURBS solid, then SHELLED to a real thin wall (caps removed
// via Pillar-3 select-face by description). Verifies ground_truth at each step.
const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";

async function req(method, path, body) {
  const r = await fetch(`${BASE}${path}`, {
    method,
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const t = await r.text();
  let j; try { j = JSON.parse(t); } catch { j = t; }
  return { ok: r.ok, status: r.status, j, t };
}
const post = (p, b) => req("POST", p, b);
const get = (p) => req("GET", p);
const del = (p) => req("DELETE", p);

const ring = (r, z, n = 56) =>
  Array.from({ length: n }, (_, i) => {
    const a = (i / n) * 2 * Math.PI;
    return [r * Math.cos(a), r * Math.sin(a), z];
  });

// Refined contour [radius, z], throat r_t = 1.0. Longer chamber, smooth
// convergence, sharp throat, long flared bell (exit r 5.2 → area ratio ~27:1),
// Rao-style: steep initial turn easing to a near-axial exit.
const CONTOUR = [
  [3.0, 0.0],    // chamber cap (planar)
  [3.0, 3.0],    // chamber
  [2.85, 4.2],   // converge start (gentle)
  [2.2, 5.4],    // converge
  [1.45, 6.2],   // converge
  [1.0, 6.8],    // THROAT (min)
  [1.3, 7.4],    // bell — steep initial expansion (~33°)
  [2.0, 8.8],    // bell
  [2.9, 10.6],   // bell easing
  [3.8, 12.8],   // bell
  [4.6, 15.2],   // bell
  [5.2, 17.5],   // exit lip (planar cap), ~27:1 area ratio
];

async function main() {
  console.log(`Refined hollow nozzle -> ${BASE}`);
  // 1. Clear the model so the scene shows only this nozzle.
  await del("/api/agent/parts");

  // 2. Build the refined bell (solid).
  const built = await post("/api/geometry/nurbs_loft", {
    sections: CONTOUR.map(([r, z]) => ring(r, z)),
    degree_u: 3, degree_v: 3, name: "Rocket Nozzle Bell",
  });
  if (!built.ok) { console.log("LOFT FAILED:", built.status, built.t.slice(0, 300)); return; }
  const uuid = built.j?.object?.id;
  console.log("loft uuid:", uuid);

  // 3. Find the solid id + resolve the two planar end caps (Pillar 3). The
  // two caps are the only PLANAR faces (the lateral is NURBS), so a planar
  // query is ambiguous and returns both candidates = the caps to open.
  const parts = (await get("/api/agent/parts")).j;
  const sid = Array.isArray(parts) && parts.length ? parts[parts.length - 1].id : 0;
  const planar = await post(`/api/agent/parts/${sid}/select-face`, { kind: "planar" });
  const faces = planar.j?.candidates ?? (planar.j?.face_id != null ? [planar.j.face_id] : []);
  console.log("cap faces (planar candidates):", JSON.stringify(faces));
  const solidTruth = (await get(`/api/agent/parts/${sid}/truth`)).j;
  console.log("solid ground_truth:", (solidTruth.summary ?? JSON.stringify(solidTruth)).slice(0, 200));

  // 4. Shell it to a real wall (open both caps).
  if (uuid && faces.length === 2) {
    const shelled = await post("/api/geometry/shell", { object: uuid, thickness: 0.3, faces_to_remove: faces });
    if (shelled.ok) {
      console.log("SHELL ok:", JSON.stringify(shelled.j).slice(0, 160));
      const np = (await get("/api/agent/parts")).j;
      const nsid = Array.isArray(np) && np.length ? np[np.length - 1].id : sid;
      const ht = (await get(`/api/agent/parts/${nsid}/truth`)).j;
      console.log("HOLLOW ground_truth:", (ht.summary ?? JSON.stringify(ht)).slice(0, 240));
    } else {
      console.log("SHELL FAILED (kept solid):", shelled.status, shelled.t.slice(0, 300));
    }
  } else {
    console.log("skip shell — caps not resolved (faces=", faces, ")");
  }
}
main().catch((e) => { console.error("ERR:", e.message); process.exit(1); });
