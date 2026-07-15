/**
 * Hub flange with GD&T + a drawing. A revolved flange (Ø60 base, Ø24 hub, Ø12
 * bore) with six Ø4 bolt holes on a r21 circle, then a certified GD&T stack:
 *   datum A = bottom face, datum B = bore axis,
 *   flatness(A face), perpendicularity(bore ⟂ A), position(bolt hole |A|B|).
 *
 * A perfectly-built part must measure IN SPEC at 0.00 on every callout; the
 * four-view drawing's quality invariants must pass.
 */
import { buildHubFlange } from "../lib/builders.mjs";

export default {
  id: "06-hub-flange-gdt",
  title: "Hub flange + GD&T (datums A/B, flatness/perp/position) + drawing",
  dims: ["correctness", "soundness", "performance"],
  budgetMs: 120000,
  async run(ctx, t) {
    const { c } = ctx;
    const { id } = await ctx.time("revolve flange + 6 bolt holes", () => buildHubFlange(c, { boltHoles: 6, boltRing: 21, boltR: 2 }));
    const per = await ctx.time("certify", () => c.perceive(id));
    t.sound("flange + 6 bolt holes certifies sound", per);
    t.eq("bore + 6 bolt holes -> chi = -12", per.euler, -12, { dim: "correctness" });

    // Resolve the datum / target faces analytically off the B-Rep.
    const feat = await c.get(`/api/agent/parts/${id}/features`);
    const bottom = feat.features.find((f) => f.surface_kind === "plane" && Math.abs(f.origin[2]) < 0.01);
    const bore = feat.features.find((f) => f.surface_kind === "cylinder" && Math.abs(f.radius - 6) < 0.01);
    const hole0 = feat.features.find(
      (f) => f.surface_kind === "cylinder" && Math.abs(f.radius - 2) < 0.01 && Math.abs(f.origin[0] - 21) < 0.2 && Math.abs(f.origin[1]) < 0.2,
    );
    t.ok("resolved bottom datum face", !!bottom, { detail: `face ${bottom?.face_id}` });
    t.ok("resolved bore datum face", !!bore, { detail: `face ${bore?.face_id}` });
    t.ok("resolved bolt hole @ (21,0)", !!hole0, { detail: `face ${hole0?.face_id}` });

    const post = (path, body) => c.raw("POST", path, body);
    // Designate datums.
    const dA = await post(`/api/agent/parts/${id}/datums`, { label: "A", face_id: bottom.face_id, selector: null });
    t.ok("datum A designated (plane)", dA.data?.success === true && dA.data?.kind === "plane");
    const dB = await post(`/api/agent/parts/${id}/datums`, { label: "B", face_id: bore.face_id, selector: null });
    t.ok("datum B designated (axis)", dB.data?.success === true && dB.data?.kind === "axis");

    // Feature control frames — each must be IN SPEC at 0.00 on a perfect part.
    const flat = await ctx.time("FCF flatness", () => post(`/api/agent/parts/${id}/fcf`, { characteristic: "flatness", tolerance_mm: 0.05, datum_refs: [], face_id: bottom.face_id, selector: null, basic: null }));
    assertInSpec(t, "flatness(A) IN SPEC 0.00", flat.data?.verdict);
    const perp = await ctx.time("FCF perpendicularity", () => post(`/api/agent/parts/${id}/fcf`, { characteristic: "perpendicularity", tolerance_mm: 0.05, datum_refs: ["A"], face_id: bore.face_id, selector: null, basic: null }));
    assertInSpec(t, "perpendicularity(bore ⟂ A) IN SPEC 0.00", perp.data?.verdict);
    const pos = await ctx.time("FCF position", () => post(`/api/agent/parts/${id}/fcf`, { characteristic: "position", tolerance_mm: 0.1, datum_refs: ["A", "B"], face_id: hole0.face_id, selector: null, basic: [21, 0] }));
    assertInSpec(t, "position(bolt |A|B| basic [21,0]) IN SPEC 0.00", pos.data?.verdict);

    // Four-view drawing with certified layout quality.
    const dr = await ctx.time("make drawing", () => c.raw("POST", `/api/parts/${id}/drawing?name=hub_flange`, undefined, 90000));
    t.eq("drawing endpoint returns 200", dr.status, 200);
    t.ok("drawing quality passed:true", dr.data?.quality?.passed === true, {
      dim: "soundness",
      detail: `passed=${dr.data?.quality?.passed}, issues=${dr.data?.quality?.issues?.length ?? 0}`,
    });
  },
};

function assertInSpec(t, name, verdict) {
  const conforms = (verdict?.conforms ?? "").toLowerCase();
  const measured = verdict?.measured_mm;
  t.ok(name, conforms === "in_spec" && Math.abs(measured ?? 9) < 1e-6, {
    detail: `conforms=${verdict?.conforms}, measured=${verdict?.measured_label ?? measured}`,
  });
}
