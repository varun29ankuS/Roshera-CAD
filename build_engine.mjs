// Thrust-chamber + Rao-nozzle wall profile (combustion barrel → converging →
// throat → 80% Rao bell), hollow thin wall, for the MCP `revolve` tool.
const Rt = 1.0, eps = 25, Re = Rt * Math.sqrt(eps), Rc = 2.5;
const thetaN = 33 * Math.PI / 180, thetaE = 9 * Math.PI / 180;
const Ru = 1.5 * Rt, Rd = 0.382 * Rt, conv = 35 * Math.PI / 180;
const Ln = 0.8 * (Rt * (Math.sqrt(eps) - 1) / Math.tan(15 * Math.PI / 180));
const t = 0.18; // wall thickness
const Lchamber = 5.0; // cylindrical combustion-chamber barrel length

const O = []; // outer meridian {r,z}, throat at z=0
const zTan = -Ru * Math.sin(conv), rTan = (Rt + Ru) - Ru * Math.cos(conv);
const zConvStart = zTan - (Rc - rTan) / Math.tan(conv); // where converging cone meets the barrel (r=Rc)
O.push({ r: Rc, z: zConvStart - Lchamber }); // chamber head
O.push({ r: Rc, z: zConvStart });            // barrel → converging
for (let i = 0; i <= 6; i++) { const phi = conv * (1 - i / 6); O.push({ r: (Rt + Ru) - Ru * Math.cos(phi), z: -Ru * Math.sin(phi) }); }
for (let i = 1; i <= 6; i++) { const phi = thetaN * i / 6; O.push({ r: (Rt + Rd) - Rd * Math.cos(phi), z: Rd * Math.sin(phi) }); }
const zN = Rd * Math.sin(thetaN), rN = (Rt + Rd) - Rd * Math.cos(thetaN), zE = zN + Ln, rE = Re;
const tn = Math.tan(thetaN), te = Math.tan(thetaE);
const zQ = ((rE - te * zE) - (rN - tn * zN)) / (tn - te), rQ = rN + tn * (zQ - zN);
for (let i = 1; i <= 20; i++) { const u = i / 20; const z = (1 - u) ** 2 * zN + 2 * (1 - u) * u * zQ + u ** 2 * zE; const r = (1 - u) ** 2 * rN + 2 * (1 - u) * u * rQ + u ** 2 * rE; O.push({ r, z }); }

// inner meridian: perpendicular inward offset by t
const I = O.map((p, i) => {
  const a = O[Math.max(0, i - 1)], b = O[Math.min(O.length - 1, i + 1)];
  let tr = b.r - a.r, tz = b.z - a.z; const L = Math.hypot(tr, tz) || 1; tr /= L; tz /= L;
  let nr = tz, nz = -tr; if (nr < 0) { nr = -nr; nz = -nz; }
  return { r: p.r - t * nr, z: p.z - t * nz };
});
const round = (x) => Math.round(x * 1000) / 1000;
// Negate z so the nozzle exit points -Z and the chamber head sits at +Z (the
// conventional engine orientation: injector on top, exhaust down). Built this way
// from the start — no post-build transform/flip, so the construction sketch never
// orphans (the divergence that the consistency invariant exists to catch).
const profile = [...O.map((p) => [round(p.r), -round(p.z)]), ...I.slice().reverse().map((p) => [round(p.r), -round(p.z)])];
console.log(JSON.stringify(profile));
console.error(`chamber+nozzle wall profile: ${profile.length} pts, t=${t}, barrel=${Lchamber}, throat z=0..exit z=${round(zE)}`);
