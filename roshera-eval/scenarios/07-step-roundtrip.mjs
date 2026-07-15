/**
 * STEP round-trip of a BLENDED flange. Build the hub flange, fillet the hub-top
 * rim (r2, a toroidal surface) and chamfer two rims (0.5, conical surfaces),
 * export AP242 STEP to disk, then re-import it by path.
 *
 * Oracle: every re-imported solid carries the FULL kernel certificate and is
 * sound:true — the blend surfaces survive the round-trip (regression guard for
 * the torus/cone import-mesher honesty fix). Exercises the production hand-off.
 */
import os from "node:os";
import path from "node:path";
import { writeFile } from "node:fs/promises";
import { BASE } from "../lib/client.mjs";
import { buildHubFlange } from "../lib/builders.mjs";

export default {
  id: "07-step-roundtrip",
  title: "STEP round-trip of the blended flange (fillet + chamfer)",
  dims: ["correctness", "soundness", "performance"],
  budgetMs: 45000,
  async run(ctx, t) {
    const { c } = ctx;
    let { uuid, id } = await ctx.time("revolve flange", () => buildHubFlange(c));

    const sel = (body) => c.raw("POST", `/api/agent/parts/${id}/select-edge`, body);
    // Fillet the hub-top rim (highest circular edge).
    const hub = (await sel({ curve_kind: "circle", blend: "any", extremal: "most_along", along: [0, 0, 1] })).data;
    const fEdge = hub.edge_id ?? hub.candidates?.[0];
    await ctx.time("fillet hub rim r2", () => c.raw("POST", "/api/geometry/fillet", { object: uuid, edges: [fEdge], radius: 2, all_edges: false }));
    id = await c.newestPartId();
    uuid = await c.uuidForPart(id);
    // Chamfer two remaining rims 0.5.
    const un = (await sel({ curve_kind: "circle", blend: "unblended", extremal: "none" })).data;
    const twoRims = (un.candidates ?? (un.edge_id != null ? [un.edge_id] : [])).slice(0, 2);
    await ctx.time("chamfer 2 rims 0.5", () => c.raw("POST", "/api/geometry/chamfer", { object: uuid, edges: twoRims, distance: 0.5 }));
    id = await c.newestPartId();
    uuid = await c.uuidForPart(id);

    const perBlend = await c.perceive(id);
    t.sound("blended flange certifies sound pre-export", perBlend);
    t.ok("carries blend faces (fillet + chamfer)", perBlend.face_count >= 8, { detail: `${perBlend.face_count} faces` });

    // Export AP242 STEP, save to a server-readable temp path.
    const ex = await ctx.time("export STEP", () => c.post("/api/export", { format: "STEP", objects: [uuid], quality: "High" }));
    t.ok("export produced a download url", !!ex?.download_url);
    const res = await fetch(BASE + ex.download_url);
    const buf = Buffer.from(await res.arrayBuffer());
    const file = path.join(os.tmpdir(), "agent_eval_blended_flange.step");
    await writeFile(file, buf);
    t.ok("STEP file written to disk", buf.length > 0, { detail: `${buf.length} bytes -> ${file}` });

    // Re-import by path.
    await c.clearParts();
    const imp = await ctx.time("re-import STEP", () => c.post("/api/geometry/import_step", { path: file, content: null, name: "reimport" }));
    const objs = imp.objects ?? [];
    t.ok("import reported success", imp.success === true, { detail: `schema=${imp.report?.schema}` });
    t.ok("at least one solid imported", objs.length >= 1, { detail: `${objs.length} solids` });
    const allSound = objs.length > 0 && objs.every((o) => o.perception?.sound === true);
    t.ok("every re-imported solid is sound:true", allSound, {
      dim: "soundness",
      detail: JSON.stringify(objs.map((o) => ({ sound: o.perception?.sound, watertight: o.perception?.watertight, manifold: o.perception?.manifold }))),
    });
  },
};
