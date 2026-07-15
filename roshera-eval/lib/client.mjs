/**
 * Thin HTTP client for the live Roshera backend (:8081) plus the small set of
 * derived reads every scenario needs (part lists, certificates, uuid lookup,
 * edge enumeration). Mirrors the request shapes the production MCP server
 * (roshera-mcp) uses so the benchmark exercises the exact agent surface.
 */

export const BASE = process.env.ROSHERA_URL ?? "http://127.0.0.1:8081";

export class HttpError extends Error {
  constructor(message, status, body) {
    super(message);
    this.status = status;
    this.body = body;
  }
}

/** Core request. Returns { status, ok, data } — never throws on 4xx/5xx so
 *  scenarios can assert on honest kernel refusals (404/409/422). Throws only on
 *  transport failure / timeout. */
export async function request(method, path, body, timeoutMs = 120000) {
  let res;
  const started = Date.now();
  try {
    res = await fetch(`${BASE}${path}`, {
      method,
      headers: {
        "X-Roshera-Agent": "AgentEvalAlpha",
        ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (err) {
    const name = err?.name;
    const ms = Date.now() - started;
    if (name === "TimeoutError" || name === "AbortError") {
      throw new HttpError(`${method} ${path} timed out after ${ms}ms`, 504, "");
    }
    throw new HttpError(`${method} ${path} network error: ${err?.message ?? err}`, 0, "");
  }
  const text = await res.text();
  let data = null;
  if (text.length) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  return { status: res.status, ok: res.ok, data };
}

/** Convenience wrappers that DO throw on non-2xx (for the happy path). */
export function makeClient(timeoutMs = 120000) {
  const call = async (method, path, body, tmo = timeoutMs) => {
    const r = await request(method, path, body, tmo);
    if (!r.ok) {
      throw new HttpError(`${method} ${path} -> ${r.status}: ${typeof r.data === "string" ? r.data : JSON.stringify(r.data)?.slice(0, 300)}`, r.status, r.data);
    }
    return r.data;
  };
  return {
    raw: (method, path, body, tmo) => request(method, path, body, tmo ?? timeoutMs),
    get: (path, tmo) => call("GET", path, undefined, tmo),
    post: (path, body, tmo) => call("POST", path, body, tmo),
    del: (path, tmo) => call("DELETE", path, undefined, tmo),
    patch: (path, body, tmo) => call("PATCH", path, body, tmo),

    async listParts() {
      const p = await call("GET", "/api/agent/parts");
      return Array.isArray(p) ? p : [];
    },
    async newestPartId() {
      const parts = await this.listParts();
      if (parts.length === 0) return null;
      return parts.reduce((mx, p) => Math.max(mx, p.id), 0);
    },
    async clearParts() {
      await call("DELETE", "/api/agent/parts");
    },
    async partReport(id) {
      return call("GET", `/api/agent/parts/${id}`);
    },
    async mass(id) {
      return call("GET", `/api/agent/parts/${id}/mass`);
    },
    /** Full certificate (?full=1) merged with the part report's structural
     *  facts into one normalized perception object. */
    async perceive(id) {
      const p = await call("GET", `/api/agent/parts/${id}/perception?full=1`);
      let report = null;
      try {
        report = await call("GET", `/api/agent/parts/${id}`);
      } catch {
        /* structural facts optional */
      }
      const cert = p?.cert ?? null;
      return {
        solid_id: id,
        sound: (p?.sound ?? p?.valid) === true,
        brep_valid: cert?.brep_valid ?? p?.valid ?? null,
        watertight: cert?.watertight ?? p?.watertight ?? null,
        manifold: cert?.manifold ?? null,
        self_intersection_free: cert?.self_intersection_free ?? null,
        oriented: cert?.oriented ?? null,
        tessellation_clean: cert?.tessellation_clean ?? null,
        mesh_quality_clean: cert?.mesh_quality_clean ?? null,
        construction_consistent: cert?.construction_consistent ?? null,
        eyes_consistent: cert?.eyes_consistent ?? null,
        euler: cert?.euler_characteristic ?? null,
        open_edges: p?.open_edges ?? cert?.boundary_edges ?? null,
        nonmanifold_edges: p?.nonmanifold_edges ?? cert?.nonmanifold_edges ?? null,
        model_debris_orphan_faces: cert?.model_debris_orphan_faces ?? null,
        face_count: report?.topology?.face_count ?? null,
        edge_count: report?.topology?.edge_count ?? null,
        volume: report?.volume ?? null,
        surface_area: report?.surface_area ?? null,
        dims: p?.dims ?? null,
        verdict: p?.verdict ?? null,
        cert,
      };
    },
    /** Resolve a kernel part id to its public object UUID via the scene snapshot. */
    async uuidForPart(id) {
      const snap = await call("GET", "/api/scene/snapshot");
      const objects = Array.isArray(snap?.objects) ? snap.objects : [];
      for (const o of objects) {
        if (o?.analytical_geometry?.solid_id === id && typeof o?.id === "string") {
          return o.id;
        }
      }
      throw new Error(`no live solid uuid for part_id ${id}`);
    },
    /** Enumerate every edge id of a solid (the kernel refuses to disambiguate and
     *  returns the full candidate set — exactly the all-edges list). */
    async allEdgeIds(id) {
      const r = await request("POST", `/api/agent/parts/${id}/select-edge`, {
        curve_kind: "any",
        blend: "any",
        extremal: "none",
      });
      const j = r.data;
      if (j?.resolved === true && typeof j.edge_id === "number") return [j.edge_id];
      if (Array.isArray(j?.candidates)) {
        return j.candidates.filter((e) => typeof e === "number");
      }
      throw new Error(`could not enumerate edges for part ${id}`);
    },
    /** Section area on a cutting plane through a part. */
    async sectionArea(id, p, normal) {
      const q = `nx=${normal[0]}&ny=${normal[1]}&nz=${normal[2]}&px=${p[0]}&py=${p[1]}&pz=${p[2]}`;
      const r = await request("GET", `/api/agent/parts/${id}/section?${q}`);
      return r; // { status, ok, data:{ section_area, ... } }
    },
  };
}
