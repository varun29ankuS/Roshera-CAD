/**
 * Shared build recipes used by more than one scenario. Every helper drives the
 * exact REST shapes the production MCP server uses (see roshera-mcp/src/tools).
 */

/** Multi-shape polyline sketch -> extrude. `shapes[0]` is the outer boundary;
 *  the rest are interior loops (holes). Returns the new kernel part id. */
export async function extrudeProfiles(c, shapes, distance, name) {
  const s = await c.post("/api/sketch", { plane: "xy", tool: "polyline" });
  const sid = s.id;
  for (const p of shapes[0]) await c.post(`/api/sketch/${sid}/point`, { point: p });
  for (let k = 1; k < shapes.length; k++) {
    const sh = await c.post(`/api/sketch/${sid}/shape`, { tool: "polyline" });
    const idx = (sh.shapes?.length ?? k + 1) - 1;
    for (const p of shapes[k]) await c.post(`/api/sketch/${sid}/shape/${idx}/point`, { point: p });
  }
  await c.post(`/api/sketch/${sid}/extrude`, { distance, name: name ?? null });
  return await c.newestPartId();
}

/** Subtract a sequence of tool cylinders from a base solid, one boolean at a
 *  time. Returns { uuid, id } of the resulting solid. Each difference consumes
 *  its operands and mints a new solid, so the uuid is re-resolved every step. */
export async function drillCylinders(c, baseUuid, holes) {
  let uuid = baseUuid;
  for (const h of holes) {
    const bore = await c.post("/api/geometry/cylinder", {
      center: h.center,
      axis: h.axis ?? [0, 0, 1],
      radius: h.radius,
      height: h.height,
      name: h.name ?? "bore",
      fast: true,
    });
    await c.post("/api/geometry/boolean", {
      operation: "difference",
      object_a: uuid,
      object_b: bore.object.id,
      fast: true,
    });
    uuid = await c.uuidForPart(await c.newestPartId());
  }
  return { uuid, id: await c.newestPartId() };
}

/** Subtract a sequence of tool boxes from a base solid. Returns { uuid, id }. */
export async function subtractBoxes(c, baseUuid, boxes) {
  let uuid = baseUuid;
  for (const b of boxes) {
    const tool = await c.post("/api/geometry/box", {
      center: b.center,
      u_axis: b.u_axis ?? [1, 0, 0],
      v_axis: b.v_axis ?? [0, 1, 0],
      width: b.width,
      depth: b.depth,
      height: b.height,
      name: b.name ?? "cut",
      fast: true,
    });
    await c.post("/api/geometry/boolean", {
      operation: "difference",
      object_a: uuid,
      object_b: tool.object.id,
      fast: true,
    });
    uuid = await c.uuidForPart(await c.newestPartId());
  }
  return { uuid, id: await c.newestPartId() };
}

/** Fillet every edge the kernel can (all_edges mode: over-radius / unblendable
 *  edges are skipped, not a whole-op refusal). Returns { status, id }. */
export async function filletAll(c, partId, radius) {
  const uuid = await c.uuidForPart(partId);
  const edges = await c.allEdgeIds(partId);
  const r = await c.raw("POST", "/api/geometry/fillet", {
    object: uuid,
    edges,
    radius,
    all_edges: true,
  });
  return { status: r.status, ok: r.ok, edgeCount: edges.length, id: await c.newestPartId() };
}

/** Build the hub flange used by the GD&T and STEP scenarios (revolve profile,
 *  then optional bolt holes). Returns { uuid, id }. */
export async function buildHubFlange(c, { boltHoles = 0, boltRing = 21, boltR = 2 } = {}) {
  const profile = [[6, 0], [30, 0], [30, 6], [12, 6], [12, 20], [6, 20]];
  const fl = await c.post("/api/geometry/revolve", {
    profile,
    axis_origin: [0, 0, 0],
    axis_direction: [0, 0, 1],
    angle_deg: 360,
    segments: 96,
    name: "flange",
  });
  let uuid = fl.object.id;
  let id = await c.newestPartId();
  if (boltHoles > 0) {
    const holes = [];
    for (let k = 0; k < boltHoles; k++) {
      const th = (2 * Math.PI * k) / boltHoles;
      holes.push({ center: [boltRing * Math.cos(th), boltRing * Math.sin(th), -1], axis: [0, 0, 1], radius: boltR, height: 8, name: "bolt" });
    }
    ({ uuid, id } = await drillCylinders(c, uuid, holes));
  }
  return { uuid, id };
}
