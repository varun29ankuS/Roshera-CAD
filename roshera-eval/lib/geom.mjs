/**
 * Deterministic geometry generators + exact analytic oracles for AGENT-EVAL-α.
 *
 * Every profile here is computed from a closed formula so the benchmark's
 * expectations are self-checking: the runner derives the expected polygon area
 * with the shoelace formula (an INDEPENDENT method) and compares it against the
 * kernel's tessellated volume integration. Agreement to a tight tolerance is a
 * genuine cross-validation of the kernel, not a hard-coded magic number.
 */

const TAU = Math.PI * 2;

/** Signed area of a closed 2-D polygon (shoelace). Positive = CCW winding. */
export function shoelaceArea(pts) {
  let a = 0;
  for (let i = 0; i < pts.length; i++) {
    const [x1, y1] = pts[i];
    const [x2, y2] = pts[(i + 1) % pts.length];
    a += x1 * y2 - x2 * y1;
  }
  return a / 2;
}

/** linspace helper. includeEnd controls whether `b` is emitted. */
function linspace(a, b, n, includeEnd = true) {
  const out = [];
  const denom = includeEnd ? n - 1 : n;
  for (let i = 0; i < n; i++) out.push(a + ((b - a) * i) / denom);
  return out;
}

/**
 * Standard external involute spur-gear outer profile (ISO 21771 geometry).
 * Returns exactly `outerPoints` [x,y] vertices forming one closed CCW loop
 * around all `z` teeth. m = module, alphaDeg = pressure angle.
 *
 * Per-tooth point budget (16 for the default 256/16 case):
 *   right involute flank (5) + tip arc (3) + left involute flank (5) + root (3).
 */
export function involuteGearProfile({
  m = 2,
  z = 16,
  alphaDeg = 20,
  outerPoints = 256,
} = {}) {
  const perTooth = outerPoints / z;
  if (!Number.isInteger(perTooth)) {
    throw new Error(`outerPoints ${outerPoints} not divisible by z ${z}`);
  }
  // Split the per-tooth budget: flanks get the lion's share, tip + root the rest.
  const nFlank = Math.max(3, Math.round((perTooth - 6) / 2) + 3 - 3); // tuned below
  // Deterministic split for the canonical 16-per-tooth budget.
  let flank = 5, tip = 3, root = 3;
  if (perTooth !== 16) {
    // Generic split: keep tip=3, root=3, remainder to the two flanks.
    tip = 3; root = 3;
    flank = Math.max(2, Math.round((perTooth - tip - root) / 2));
    // Adjust root so the total lands exactly on perTooth.
    root = perTooth - 2 * flank - tip;
    if (root < 1) { flank -= 1; root = perTooth - 2 * flank - tip; }
  }
  void nFlank;

  const alpha = (alphaDeg * Math.PI) / 180;
  const rp = (m * z) / 2; // pitch radius
  const rb = rp * Math.cos(alpha); // base radius
  const ra = rp + m; // addendum (tip) radius
  const rf = rp - 1.25 * m; // dedendum (root) radius
  const invAlpha = Math.tan(alpha) - alpha; // inv(alpha)
  const C = Math.PI / (2 * z) + invAlpha; // half tooth angle referenced to base

  // Involute unwrap angle at radius r (>= rb).
  const inv = (r) => {
    const a = Math.acos(Math.min(1, rb / r));
    return Math.tan(a) - a;
  };
  // Half-flank angle from the tooth centreline at radius r.
  const flankAngle = (r) => C - inv(r);

  const pts = [];
  for (let i = 0; i < z; i++) {
    const th = (i * TAU) / z; // tooth centre angle
    // Right flank: rb -> ra (endpoint excluded; tip covers ra).
    for (const r of linspace(rb, ra, flank, false)) {
      const ang = th - flankAngle(r);
      pts.push([r * Math.cos(ang), r * Math.sin(ang)]);
    }
    // Tip arc across the top at r = ra (both corners inclusive).
    const faTip = flankAngle(ra);
    for (const ang of linspace(th - faTip, th + faTip, tip, true)) {
      pts.push([ra * Math.cos(ang), ra * Math.sin(ang)]);
    }
    // Left flank: ra -> rb (start excluded; rb inclusive).
    const leftR = linspace(ra, rb, flank + 1, true).slice(1); // drop ra, keep rb
    for (const r of leftR) {
      const ang = th + flankAngle(r);
      pts.push([r * Math.cos(ang), r * Math.sin(ang)]);
    }
    // Root gap at r = rf, from this tooth's left edge to the next tooth's right.
    const thNext = ((i + 1) * TAU) / z;
    const a0 = th + C;
    const a1 = thNext - C;
    for (const ang of linspace(a0, a1, root, true)) {
      pts.push([rf * Math.cos(ang), rf * Math.sin(ang)]);
    }
  }
  // Guard: exact count.
  if (pts.length !== outerPoints) {
    throw new Error(
      `gear profile produced ${pts.length} points, expected ${outerPoints}`,
    );
  }
  return { pts, rp, rb, ra, rf };
}

/**
 * DIN 6885-style keyed bore as ONE closed polyline of exactly `points` vertices:
 * a circle of radius `rBore` with a rectangular keyway slot of half-width
 * `keyHalfW` cut out to radius `notchTop` at the top (+Y).
 *
 * The keyway walls and top are SAMPLED with several vertices each (`keyPts`),
 * not left as four sharp corners. A four-corner notch is a clean standalone
 * polygon but its sharp re-entrant corners make the kernel's hole triangulator
 * mis-classify the region when it is used as an interior loop; the intermediate
 * vertices keep the constrained triangulation well-conditioned.
 */
export function keyedBoreProfile({
  rBore = 5,
  keyHalfW = 1.5,
  notchTop = 6.4,
  points = 35,
  keyPts = 9,
} = {}) {
  const yBase = Math.sqrt(rBore * rBore - keyHalfW * keyHalfW);
  // Split the keyway vertex budget across the two walls and the top.
  const perWall = Math.max(2, Math.round((keyPts - 1) / 3));
  const topPts = Math.max(1, keyPts - 2 * perWall);
  // Right wall: base -> top (ascending y), inclusive of both.
  const rightWall = linspace(yBase, notchTop, perWall, true).map((y) => [keyHalfW, y]);
  // Top: across x from +keyHalfW side to -keyHalfW side, INTERIOR points only
  // (the wall tops already sit at the corners).
  const top = [];
  for (let i = 1; i <= topPts; i++) {
    const x = keyHalfW - (2 * keyHalfW * i) / (topPts + 1);
    top.push([x, notchTop]);
  }
  // Left wall: top -> base (descending y), inclusive of both.
  const leftWall = linspace(notchTop, yBase, perWall, true).map((y) => [-keyHalfW, y]);
  const key = [...rightWall, ...top, ...leftWall];

  // Circle arc the LONG way (CCW through the bottom) from the left-base corner
  // back to the right-base corner, excluding both ends (walls own them).
  const angRight = Math.atan2(yBase, keyHalfW);
  const angLeft = Math.atan2(yBase, -keyHalfW);
  const arcPts = points - key.length;
  if (arcPts < 3) throw new Error(`keyed bore: keyPts ${keyPts} too large for points ${points}`);
  const arc = [];
  for (let i = 1; i <= arcPts; i++) {
    const a = angLeft + ((angRight + TAU - angLeft) * i) / (arcPts + 1);
    arc.push([rBore * Math.cos(a), rBore * Math.sin(a)]);
  }
  const pts = [...key, ...arc];
  if (pts.length !== points) {
    throw new Error(`bore profile produced ${pts.length} points, expected ${points}`);
  }
  return { pts, rBore, notchTop, keyHalfW };
}

/** Volume of a straight prism = |cross-section area| x height. */
export function prismVolume(areaMm2, heightMm) {
  return Math.abs(areaMm2) * heightMm;
}

/** Steel mass (kg) from volume (mm^3): rho = 7850 kg/m^3, 1 mm^3 = 1e-9 m^3. */
export function steelMassKg(volMm3, density = 7850) {
  return volMm3 * 1e-9 * density;
}
