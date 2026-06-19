#!/usr/bin/env node
// Build a realistic F1 car as a multi-part assembly via the Roshera REST API.
//  - Body:       single G2 NURBS loft (needle nose -> cockpit -> airbox -> tail)
//  - Tyres:      revolved tyre cross-section (flat tread + shoulders + hub bore)
//  - Rims:       cylinder filling the hub bore
//  - Wings:      true airfoil sections lofted across the span + endplates
//  - Suspension: thin cylinders (wishbones) body -> wheel hub
// Units: mm. Scale S keeps it inside the viewport's auto-frame.
const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";
const S = Number(process.env.F1_SCALE ?? 0.08);
const PI = Math.PI;

async function api(path, body, method = "POST") {
  const r = await fetch(`${BASE}${path}`, {
    method,
    headers: { "Content-Type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  const txt = await r.text();
  let j; try { j = JSON.parse(txt); } catch { j = { raw: txt }; }
  if (!r.ok) throw new Error(`${path} -> HTTP ${r.status}: ${txt.slice(0, 200)}`);
  return j;
}
const ok = (label, j) => {
  const p = j.perception ?? {};
  console.log(`  ${label.padEnd(16)} solid=${j.solid_id ?? "?"} tris=${j.stats?.triangle_count ?? "?"} valid=${p.valid ?? "-"} wt=${p.watertight ?? "-"}`);
  return j;
};
const sc = (a) => a.map((v) => v * S);

// ---- geometry helpers --------------------------------------------------
// Rounded-rectangle (superellipse) ring in the YZ plane at length x. `notch`
// dips the top-centre down (mm) to carve a cockpit trough along the loft.
function ring(x, w, h, zf, n = 40, e = 0.5, notch = 0) {
  const pts = [];
  for (let i = 0; i < n; i++) {
    const t = (i * 2 * PI) / n, c = Math.cos(t), s = Math.sin(t);
    const cy = Math.sign(c) * Math.pow(Math.abs(c), e);
    const cz = Math.sign(s) * Math.pow(Math.abs(s), e);
    let z = zf + 0.5 * h * (1 + cz);
    if (notch > 0 && s > 0) z -= notch * s * (1 - Math.abs(cy)); // central top dip
    pts.push([x * S, w * S * cy, z * S]);
  }
  return pts;
}
// Ellipse ring in the YZ plane at length x, centred (cy,cz), half-axes ry,rz.
function ellipseRing(x, cy, cz, ry, rz, n = 28) {
  const pts = [];
  for (let i = 0; i < n; i++) {
    const t = (i * 2 * PI) / n;
    pts.push([x * S, (cy + ry * Math.cos(t)) * S, (cz + rz * Math.sin(t)) * S]);
  }
  return pts;
}
// Closed airfoil loop in (chordX, z); NACA-ish thickness + sinusoidal camber.
function airfoil(chord, thick, camber, n = 18) {
  const up = [], lo = [];
  for (let i = 0; i <= n; i++) {
    const t = i / n, xx = t * chord;
    const th = thick * (0.2969 * Math.sqrt(t) - 0.126 * t - 0.3516 * t * t + 0.2843 * t ** 3 - 0.1015 * t ** 4) / 0.2;
    const cz = camber * Math.sin(PI * t);
    up.push([xx, cz + th]); lo.push([xx, cz - th]);
  }
  return [...up, ...lo.slice(1, -1).reverse()]; // closed
}

async function box(name, w, h, d, t) {
  const c = await api("/api/geometry", { shape_type: "box", parameters: { width: w * S, height: h * S, depth: d * S } });
  if (c.object?.id) await api("/api/geometry/transform", { object: c.object.id, translation: sc(t) });
  return ok(name, c);
}
async function cyl(name, center, axis, r, h) {
  return ok(name, await api("/api/geometry/cylinder", { center: sc(center), axis, radius: r * S, height: h * S, name }));
}

// A wing = airfoil sections lofted across the span (Y). Tips planar -> caps.
async function wing(name, { span, chord, thick, camber, x0, z0, stations = 5 }) {
  const sections = [];
  for (let k = 0; k < stations; k++) {
    const y = -span + (2 * span * k) / (stations - 1);
    const af = airfoil(chord, thick, camber);
    sections.push(af.map(([cx, cz]) => [ (x0 + cx) * S, y * S, (z0 + cz) * S ]));
  }
  return ok(name, await api("/api/geometry/nurbs_loft", { sections, degree_u: 3, degree_v: 2, name }));
}

// A tyre = revolved cross-section (about +Z), rotated onto the Y axis, placed.
async function tyre(name, xWheel, ySign, { rt = 360, rh = 150, width = 380, track = 800 }) {
  const profile = [
    [rh, 0], [rt - 35, 0], [rt, 40], [rt + 6, width / 2], [rt, width - 40],
    [rt - 35, width], [rh, width],
  ].map(([r, z]) => [r * S, z * S]);
  const rev = await api("/api/geometry/revolve", { profile, axis_origin: [0, 0, 0], axis_direction: [0, 0, 1], segments: 48, name });
  // Revolved about +Z: axis Z, width along +Z (0..width), centred on origin.
  // Rotate +90° about X (Z -> -Y), then translate so the wheel sits centred at
  // (xWheel, ySign*track, rt) on the ground.
  const uuid = rev.object?.id;
  if (uuid) {
    await api("/api/geometry/transform", { object: uuid, rotation: { axis: [1, 0, 0], angle: PI / 2, center: [0, 0, 0] } });
    // After rotation the tyre spans y in [-width,0], centred near y=-width/2, axis Y.
    await api("/api/geometry/transform", { object: uuid, translation: sc([xWheel, ySign * track + width / 2, rt]) });
  }
  return ok(name, rev);
}

async function main() {
  console.log(`F1 (realistic) -> ${BASE}  scale=${S}`);
  await api("/api/agent/parts", null, "DELETE").then(() => console.log("  scene cleared"));

  // 1) Body — needle nose, cockpit, airbox hump, coke-bottle tail.
  // F1 plan: narrow pointed nose held narrow, sharp widen to the sidepods, then
  // coke-bottle in to a slim tail. (x, halfWidth, height, floorZ)
  // (x, halfWidth, height, floorZ, cockpitNotch) — notch carves the cockpit
  // trough into the top over the cockpit stations.
  const STN = [
    [0, 16, 36, 92, 0], [400, 44, 58, 80, 0], [900, 70, 95, 60, 0], [1400, 95, 175, 42, 55],
    [1750, 175, 300, 34, 170], [2150, 400, 380, 28, 175], [2600, 440, 400, 26, 70],
    [3100, 420, 360, 26, 0], [3550, 320, 305, 28, 0], [4050, 205, 245, 30, 0],
    [4600, 140, 185, 34, 0], [5050, 90, 130, 40, 0], [5400, 32, 80, 52, 0],
  ];
  const bodyR = await api("/api/geometry/nurbs_loft", {
    sections: STN.map(([x, w, h, zf, nt]) => ring(x, w, h, zf, 44, 0.4, nt)), degree_u: 3, degree_v: 3, name: "F1 Body",
  });
  ok("body", bodyR);
  let bodyUuid = bodyR.object?.id;

  // 2) Tyres + rims (Ø720, width 380), front & rear, both sides.
  const TRACK = 800, RT = 360, RH = 150, WW = 380;
  for (const [lab, x] of [["F", 1100], ["R", 4150]]) {
    for (const sgn of [+1, -1]) {
      await tyre(`tyre_${lab}${sgn > 0 ? "L" : "R"}`, x, sgn, { rt: RT, rh: RH, width: WW, track: TRACK });
      const baseY = sgn * TRACK - WW / 2;
      await cyl(`rim_${lab}${sgn > 0 ? "L" : "R"}`, [x, baseY, RT], [0, 1, 0], RH + 8, WW);
    }
  }

  // 2b) Airbox intake — a tapering scoop loft on the engine cover behind the
  //     cockpit (rounded mouth → into the engine cover).
  ok("airbox", await api("/api/geometry/nurbs_loft", {
    sections: [
      ellipseRing(2150, 0, 470, 105, 95),
      ellipseRing(2500, 0, 470, 95, 82),
      ellipseRing(2900, 0, 430, 64, 55),
    ],
    degree_u: 3, degree_v: 2, name: "Airbox",
  }));

  // 2c) Halo — central front pillar + two rails arcing back over the cockpit
  //     sides (the Y-shape of the modern halo).
  await cyl("halo_pillar", [1520, 0, 250], [0, 0, 1], 16, 130);
  const haloRail = async (name, sgn) => {
    const a = [1520, 0, 372], b = [2200, sgn * 155, 345];
    const d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]], L = Math.hypot(...d);
    await cyl(name, a, [d[0] / L, d[1] / L, d[2] / L], 14, L);
  };
  await haloRail("halo_rail_L", +1);
  await haloRail("halo_rail_R", -1);

  // 2d) Floor — wide flat plank under the car (grounds it; reads as the F1 floor).
  await box("floor", 3800, 880, 26, [2700, 0, 15]);

  // 2e) Rear diffuser — upswept wedge under the tail.
  {
    const d = await api("/api/geometry", { shape_type: "box", parameters: { width: 900 * S, height: 780 * S, depth: 120 * S } });
    if (d.object?.id) await api("/api/geometry/transform", { object: d.object.id, rotation: { axis: [0, 1, 0], angle: -0.22, center: [0, 0, 0] }, translation: sc([4980, 0, 120]) });
    ok("diffuser", d);
  }

  // 2f) Cockpit mirrors — small pods either side of the cockpit.
  await box("mirror_L", 70, 95, 48, [2060, 215, 360]);
  await box("mirror_R", 70, 95, 48, [2060, -215, 360]);

  // 2g) Bargeboards — vertical turning vanes ahead of the sidepods.
  await box("barge_L", 430, 16, 235, [1640, 305, 150]);
  await box("barge_R", 430, 16, 235, [1640, -305, 150]);

  // 2h) Brake discs — thin discs inside each wheel.
  for (const [lab, x] of [["F", 1100], ["R", 4150]]) {
    for (const sgn of [+1, -1]) {
      await cyl(`brake_${lab}${sgn > 0 ? "L" : "R"}`, [x, sgn * 800 - 16, 360], [0, 1, 0], 112, 32);
    }
  }

  // 2i) Shark-fin engine cover — thin vertical fin along the centreline.
  ok("sharkfin", await api("/api/geometry/nurbs_loft", {
    sections: [
      ellipseRing(2950, 0, 320, 9, 90),
      ellipseRing(3700, 0, 300, 8, 110),
      ellipseRing(4500, 0, 250, 7, 120),
      ellipseRing(5150, 0, 170, 6, 95),
    ],
    degree_u: 3, degree_v: 3, name: "Shark Fin",
  }));

  // 2j) Front-wing support pylon (nose → wing) + rear crash/exhaust tube.
  await box("fw_pylon", 230, 36, 120, [205, 0, 95]);
  await cyl("exhaust", [5180, 0, 230], [1, 0, 0.06], 32, 360);

  // 3) Front wing — main plane + flap, LOW and WIDE at the nose, vertical
  //    endplates (thin in Y, tall in Z, long in X).
  await wing("fw_main", { span: 900, chord: 500, thick: 45, camber: 30, x0: -40, z0: 45, stations: 5 });
  await wing("fw_flap", { span: 880, chord: 260, thick: 32, camber: 48, x0: 360, z0: 100, stations: 5 });
  await box("fw_ep_L", 560, 18, 250, [230, 905, 150]);
  await box("fw_ep_R", 560, 18, 250, [230, -905, 150]);

  // 4) Rear wing — swan-neck pylon + main + flap, HIGH at the tail, big
  //    vertical endplates.
  await box("rw_pylon", 150, 70, 780, [5010, 0, 560]);
  await wing("rw_main", { span: 470, chord: 360, thick: 45, camber: 35, x0: 5080, z0: 880, stations: 4 });
  await wing("rw_flap", { span: 460, chord: 210, thick: 30, camber: 52, x0: 5370, z0: 985, stations: 4 });
  await box("rw_ep_L", 520, 22, 360, [5240, 478, 905]);
  await box("rw_ep_R", 520, 22, 360, [5240, -478, 905]);

  // 4) Suspension wishbones — thin cylinders body -> wheel hub.
  const arm = async (name, x, sgn, zIn, zOut) => {
    // Inboard end penetrates the chassis (body half-width ~120); outboard end
    // reaches into the wheel hub — so the wishbone visibly bridges body↔wheel.
    const inb = [x, sgn * 70, zIn], out = [x, sgn * (TRACK - WW / 2 + 60), zOut];
    const d = [out[0] - inb[0], out[1] - inb[1], out[2] - inb[2]];
    const L = Math.hypot(...d);
    await cyl(name, inb, [d[0] / L, d[1] / L, d[2] / L], 16, L);
  };
  for (const [lab, x] of [["F", 1100], ["R", 4150]]) {
    for (const sgn of [+1, -1]) {
      await arm(`susp_${lab}${sgn > 0 ? "L" : "R"}_lo`, x, sgn, 120, 300);
      await arm(`susp_${lab}${sgn > 0 ? "L" : "R"}_up`, x, sgn, 320, 420);
    }
  }

  console.log("F1 (realistic) assembly complete.");
}
main().catch((e) => { console.error("FAILED:", e.message); process.exit(1); });
