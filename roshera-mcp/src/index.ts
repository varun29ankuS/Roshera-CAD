#!/usr/bin/env node
/**
 * Roshera MCP server — the agent-facing tool surface over the Roshera
 * geometry kernel's REST API.
 *
 * Design doctrine (from the 2026-06-12 live sessions):
 *  - LATENCY: batch tools (`sketch_polygon` takes all points in one call)
 *    and composite tools (`create_cylinder` = sketch+points+extrude in one
 *    call) collapse what previously took N round trips into one.
 *  - PERCEPTION: `render_part` returns the image as MCP image content —
 *    the agent SEES the geometry directly in the tool result. `mode:"ids"`
 *    is set-of-marks for topology (flat color per face + legend).
 *  - SHARED ATTENTION: `get_pointer` reads what the human is pointing at
 *    in the viewport (the HOVER-α bridge), so "this face" grounds against
 *    real topology.
 *  - PLACEMENT IS EXPLICIT: every create tool takes coordinates; every
 *    result echoes the part's world placement.
 *
 * Server URL via ROSHERA_URL (default http://localhost:8081).
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";

const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";

// ─── HTTP helpers ──────────────────────────────────────────────────────

class ApiError extends Error {
  constructor(message: string, public status: number, public body: string) {
    super(message);
  }
}

// Per-request timeout. A heavy kernel op (boolean over a complex part, fine
// tessellation, full re-cert) can legitimately take many seconds; the default
// is generous so we never abort a real computation, but it is bounded so a
// genuinely wedged backend surfaces as a clear 504 rather than hanging the
// agent forever. Override per process with ROSHERA_MCP_TIMEOUT_MS.
const TIMEOUT_MS = (() => {
  const raw = process.env.ROSHERA_MCP_TIMEOUT_MS;
  const n = raw !== undefined ? Number(raw) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 120000;
})();

// AMBIENT-PERCEPTION timeout — a SHORT, separate budget for the best-effort
// perception fetches (`/perception`, the part GET, the X-ray, the render) that
// run after every mutating op. These are advisory: a slow or wedged perception
// must NEVER hang the op the agent actually requested. Bounded tight so the op
// result returns promptly even if the perception layer is slow; on timeout the
// perception is simply omitted (the op result still stands). Override with
// ROSHERA_MCP_PERCEPTION_TIMEOUT_MS.
const PERCEPTION_TIMEOUT_MS = (() => {
  const raw = process.env.ROSHERA_MCP_PERCEPTION_TIMEOUT_MS;
  const n = raw !== undefined ? Number(raw) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 4000;
})();

async function api(
  method: "GET" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
  timeoutMs: number = TIMEOUT_MS,
): Promise<any> {
  let res: Response;
  try {
    res = await fetch(`${BASE}${path}`, {
      method,
      headers: {
        // Timeline attribution: the backend's agent_author_layer records
        // every kernel op from this request as Author::AIAgent("Claude"),
        // so agent-built features show amber Ⓒ in the Timeline strip.
        "X-Roshera-Agent": "Claude",
        ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
      // AbortSignal.timeout fires a TimeoutError after the budget; older
      // runtimes surface the abort as AbortError. Either way we map it to a
      // 504 so the agent gets an actionable message, not a raw stack. The
      // ambient-perception fetches pass the short PERCEPTION_TIMEOUT_MS.
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (err) {
    const name = (err as { name?: string })?.name;
    if (name === "TimeoutError" || name === "AbortError") {
      throw new ApiError(
        `${method} ${path} → timed out after ${timeoutMs}ms (backend may still be computing a heavy op; raise ROSHERA_MCP_TIMEOUT_MS)`,
        504,
        "",
      );
    }
    const msg = err instanceof Error ? err.message : String(err);
    throw new ApiError(`${method} ${path} → network error: ${msg}`, 0, "");
  }
  const text = await res.text();
  if (!res.ok) {
    throw new ApiError(
      `${method} ${path} → ${res.status}: ${text}`,
      res.status,
      text,
    );
  }
  const parsed = text.length ? JSON.parse(text) : null;
  // EMBEDDED-PERCEPTION REUSE (no redundant round-trip / no double cert). Every
  // mutating geometry endpoint already embeds its CHEAP perception verdict
  // (brep_valid, watertight/open_edges, dims, volume, face_count — and the FULL
  // `cert` only on the explicit `verify:true` opt-in path). Stash it so the
  // following perceive() reuses THIS verdict instead of firing a second
  // GET /perception. We only stash for mutating verbs; GETs (including
  // /perception itself) never overwrite the stash.
  if (method !== "GET" && parsed && typeof parsed === "object") {
    const embedded = perceptionFromBody(parsed);
    if (embedded !== undefined) {
      lastEmbeddedPerception = {
        id: parsed.solid_id ?? parsed.id ?? null,
        perception: embedded,
      };
    }
  }
  return parsed;
}

// ─── Embedded-perception reuse (no double certification) ───────────────

/**
 * The perception verdict carried by the most recent mutating response.
 * `perceive()` consumes this in preference to re-fetching /perception, so the
 * agent sees the SAME verdict the REST op computed — never a redundant re-fetch.
 */
let lastEmbeddedPerception: { id: number | null; perception: any } | null = null;

/**
 * Project a raw mutating response into the shape `perceive()` returns, reusing
 * the verdict the endpoint already embedded.
 *
 * The DEFAULT (sub-second) op response carries the CHEAP verdict inline
 * (`sound`/`valid`, `watertight`, `open_edges`, `dims`, `volume`, `face_count`)
 * and NO `cert`. The explicit `verify:true` opt-in additionally embeds the FULL
 * `cert`. We build a perception from whichever is present, preferring the full
 * cert's fields when it is. Returns `undefined` only when the response carries no
 * usable verdict at all (a server too old to perceive) — then the caller falls
 * back to the live GET /perception fetch (which is itself cheap by default).
 *
 * The expensive certificate dimensions (manifold, self_intersection_free,
 * tessellation/mesh-quality) are present ONLY when a full `cert` was embedded;
 * otherwise they are reported `null`, signalling "not computed on the hot path —
 * call verify_part / ground_truth to certify". They are never fabricated.
 */
function perceptionFromBody(r: any): any {
  if (!r || typeof r !== "object") return undefined;
  const cert = r.cert ?? r.perception?.cert ?? null;
  const soundRaw = r.sound ?? r.perception?.sound;
  const validRaw = r.valid ?? r.perception?.valid;
  // Nothing to reuse — let perceive() fetch /perception.
  if (cert === null && soundRaw === undefined && validRaw === undefined) {
    return undefined;
  }
  const sound = (soundRaw ?? validRaw) === true;
  return {
    sound,
    brep_valid: cert?.brep_valid ?? validRaw ?? null,
    watertight: cert?.watertight ?? r.watertight ?? r.perception?.watertight ?? null,
    // Full-cert-only dimensions: null when no cert was embedded (cheap path) —
    // explicitly "not certified on the hot path", never a fabricated verdict.
    manifold: cert?.manifold ?? null,
    self_intersection_free: cert?.self_intersection_free ?? null,
    construction_consistent: cert?.construction_consistent ?? null,
    labels_consistent: cert?.labels_consistent ?? null,
    tessellation_clean: cert?.tessellation_clean ?? null,
    mesh_quality_clean: cert?.mesh_quality_clean ?? null,
    euler_characteristic: cert?.euler_characteristic ?? null,
    // Dual-eye gate — null on cheap hot path (cert not run), real tri-state when
    // full cert is embedded (verify:true opt-in). Never fabricated.
    eyes_consistent: cert?.eyes_consistent ?? null,
    open_edges: r.open_edges ?? r.perception?.open_edges ?? cert?.boundary_edges ?? null,
    nonmanifold_edges:
      r.nonmanifold_edges ?? r.perception?.nonmanifold_edges ?? cert?.nonmanifold_edges ?? null,
    dims: r.dims ?? r.perception?.dims ?? null,
    // Cheap structural facts the op now returns inline; backfilled by perceive()
    // from a light part GET only if absent.
    face_count: r.face_count ?? r.perception?.face_count ?? null,
    volume: r.volume ?? r.perception?.volume ?? null,
    errors: cert?.errors ?? null,
    cert: cert ?? undefined,
    verdict:
      (r.verdict ?? r.perception?.verdict) ??
      (sound ? "OK — valid closed solid (cheap verdict; verify_part to certify)" : "UNSOUND — see verify_part"),
  };
}

function ok(data: unknown) {
  const content: any[] = [
    { type: "text" as const, text: JSON.stringify(data, null, 2) },
  ];
  return { content };
}

function fail(e: unknown) {
  const msg = e instanceof Error ? e.message : String(e);
  const hint = errorHint(msg);
  return {
    content: [
      {
        type: "text" as const,
        text: hint ? `ERROR: ${msg}\nHINT: ${hint}` : `ERROR: ${msg}`,
      },
    ],
    isError: true as const,
  };
}

/**
 * Translate a common kernel refusal into ONE actionable next step. The kernel
 * refuses rather than ship bad geometry (the moat); this turns its terse,
 * correct error into guidance the agent can act on. Returns null when the raw
 * message is already clear.
 */
function errorHint(msg: string): string | null {
  const m = msg.toLowerCase();
  if (
    m.includes("invalidradius") ||
    (m.includes("radius") && m.includes("not greater"))
  )
    return "radius is non-positive or larger than an edge's available corner room — retry with a smaller radius, or pass explicit edge_ids to blend only the edges that fit.";
  if (m.includes("self-intersect") || m.includes("self intersect"))
    return "the result would self-intersect — reduce the radius/distance, or apply the blend to fewer edges.";
  if (m.includes("not found in any face") || m.includes("3-valent corner"))
    return "an edge could not be blended at a degenerate corner — try a smaller radius or a subset of edges; if it persists the part topology needs healing.";
  if (
    m.includes("unsound") ||
    m.includes("non-manifold") ||
    m.includes("not certified")
  )
    return "the kernel produced an unsound result and refused it (the moat held) — inspect with verify_part / ground_truth; do NOT assume the geometry is valid.";
  if (m.includes("no live solid") || m.includes("not found"))
    return "the part_id may be stale or consumed (booleans consume their operands) — call list_parts for the current ids.";
  return null;
}

/** Fetch a part's placement so create-tools can echo where things landed. */
async function placement(partId: number) {
  try {
    const r = await api("GET", `/api/agent/parts/${partId}`);
    return {
      center_world: r?.location?.center_world ?? null,
      dimensions_world: r?.location?.dimensions_world ?? null,
    };
  } catch {
    return null;
  }
}

async function newestPartId(): Promise<number | null> {
  const parts = await api("GET", "/api/agent/parts");
  if (!Array.isArray(parts) || parts.length === 0) return null;
  return parts.reduce((m: number, p: any) => Math.max(m, p.id), 0);
}

/**
 * Resolve a kernel integer part_id to its public object UUID.
 *
 * The `/api/geometry/{fillet,chamfer,shell,…}` endpoints address solids by the
 * public UUID (`object` field), not the kernel SolidId the agent surface speaks
 * in (`list_parts`, `render_part`, every `/api/agent/parts/{id}` route). The
 * UUID↔SolidId map lives only in the backend's AppState and is never returned by
 * an agent route, so we recover it from the scene snapshot — every object there
 * carries both `id` (UUID) and `analytical_geometry.solid_id` (the integer id).
 * Throws a clear error when no live solid matches, so the tool fails loudly
 * instead of POSTing a bogus `object`.
 */
async function uuidForPart(partId: number): Promise<string> {
  const snap = await api("GET", "/api/scene/snapshot");
  const objects = Array.isArray(snap?.objects) ? snap.objects : [];
  for (const o of objects) {
    if (o?.analytical_geometry?.solid_id === partId && typeof o?.id === "string") {
      return o.id;
    }
  }
  throw new Error(
    `no live solid found for part_id ${partId} (run list_parts to see current ids)`,
  );
}

/**
 * Enumerate EVERY edge id of a solid via the agent select-edge endpoint with the
 * widest possible query (`curve_kind:any`, `blend:any`, no extremal). For a real
 * solid (>1 edge) the kernel REFUSES to pick one and returns the full candidate
 * set as an `ambiguous` 409 — which is exactly the all-edges list we want. A
 * single-edge solid resolves directly. The blend tools use this for their
 * all-edges mode (omitted `edge_ids`).
 */
async function allEdgeIds(partId: number): Promise<number[]> {
  const res = await fetch(`${BASE}/api/agent/parts/${partId}/select-edge`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ curve_kind: "any", blend: "any", extremal: "none" }),
  });
  const j: any = await res.json().catch(() => null);
  if (j?.resolved === true && typeof j.edge_id === "number") return [j.edge_id];
  if (Array.isArray(j?.candidates)) {
    return j.candidates.filter((e: unknown): e is number => typeof e === "number");
  }
  throw new Error(
    `could not enumerate edges for part_id ${partId}` +
      (j?.message ? `: ${j.message}` : ""),
  );
}

/**
 * STRUCTURE channel: attach the SDF occupancy X-ray (slice-stack of '#'/'.', n=16)
 * to a perception object — reveals internal cavities, wall thickness and through-
 * holes the validity verdict and a shaded render can't show. Sampled from the
 * kernel's EXACT solid, so it can't be fooled by tessellation.
 *
 * LATENCY: the X-ray is an n³ SDF sample (n=16 → 4096 exact point-in-solid
 * tests), too expensive to run after EVERY mutating op. It is therefore OFF the
 * ambient hot path — `perceive()` no longer calls it. It runs only on the
 * explicit `occupancy_view` tool, or ambiently when the operator opts in with
 * `ROSHERA_AMBIENT_PERCEPTION=xray`. Best-effort + short timeout: a slow/failed
 * X-ray just omits itself; it can never hang the op.
 */
async function addOccupancyXray(target: Record<string, any>, partId: number): Promise<void> {
  try {
    const occ = await api(
      "GET",
      `/api/agent/parts/${partId}/occupancy?n=16`,
      undefined,
      PERCEPTION_TIMEOUT_MS,
    );
    if (occ?.slices !== undefined) {
      target.occupancy_xray = occ.slices;
      target.fill_fraction = occ.fill_fraction ?? null;
    }
  } catch {
    // omit the X-ray; cert stands
  }
}

/**
 * Automatic perception — the ambient default. After any mutating op, fetch the
 * result part's FULL soundness certificate + structural facts so the agent never
 * operates blind. `/perception` now returns the full kernel certificate by
 * default (the api-server runs `certify_solid` in its bounded/coarse mode), so
 * `sound` here is the AUTHORITATIVE full verdict — brep_valid ∧ watertight ∧
 * manifold ∧ self-intersection-free ∧ construction-consistent ∧ tessellation-
 * clean ∧ mesh-quality-clean — not the shallow B-Rep-only signal. Face-count /
 * volume come from the part query. Default-ON; disable per process with
 * `ROSHERA_MCP_AUTOVERIFY=0`. Best-effort: returns `undefined` (no perception
 * block, never an error) if anything fails, so it can't break a real result.
 */
async function perceive(partId: number | null): Promise<any> {
  if (partId === null || process.env.ROSHERA_MCP_AUTOVERIFY === "0") {
    return undefined;
  }
  try {
    // FAST PATH (no double certification): the mutating op that produced this
    // part ALREADY ran the full certificate and embedded it in its response,
    // which api() stashed. Reuse it verbatim — the `sound`/`cert` surfaced here
    // are byte-identical to what the REST op computed. We never re-run
    // certify_solid. The stash matches when its id equals partId, or when the
    // op did not report a solid_id (id === null) — in which case this single
    // in-flight perception is unambiguously for the part we just touched.
    if (
      lastEmbeddedPerception &&
      (lastEmbeddedPerception.id === partId || lastEmbeddedPerception.id === null)
    ) {
      const p = lastEmbeddedPerception.perception;
      lastEmbeddedPerception = null;
      // Backfill face_count/volume only when the embedded perception didn't
      // already carry them (the cheap O(n) verdict now does). ONE light part GET
      // (read lock, no cert), short timeout — never blocks the op.
      if (p.face_count == null || p.volume == null) {
        const part = await api(
          "GET",
          `/api/agent/parts/${partId}`,
          undefined,
          PERCEPTION_TIMEOUT_MS,
        ).catch(() => null);
        if (p.face_count == null) p.face_count = part?.topology?.face_count ?? null;
        if (p.volume == null) p.volume = part?.volume ?? null;
      }
      if (process.env.ROSHERA_AMBIENT_PERCEPTION === "xray") {
        await addOccupancyXray(p, partId);
      }
      return p;
    }
    // FALLBACK CHEAP-VERDICT channel: GET /perception (default) is the CHEAP,
    // sub-second verdict — B-Rep validity + coarse mesh counts + dims, no O(n²)
    // certificate. `cert` is absent here (it's the explicit verify_part /
    // ground_truth path now), so manifold / self-intersection / mesh-quality
    // report `null` = "not certified on the hot path". Short timeout: a slow
    // perception is omitted, never blocks the op.
    const p = await api(
      "GET",
      `/api/agent/parts/${partId}/perception`,
      undefined,
      PERCEPTION_TIMEOUT_MS,
    );
    const part = await api(
      "GET",
      `/api/agent/parts/${partId}`,
      undefined,
      PERCEPTION_TIMEOUT_MS,
    ).catch(() => null);
    const cert = p?.cert ?? null;
    // `sound` is the full verdict when a cert is present (only via ?full), else
    // the cheap B-Rep validity flag.
    const sound = (p?.sound ?? p?.valid) === true;
    const brepValid = cert?.brep_valid ?? p?.valid ?? null;
    const watertight = cert?.watertight ?? p?.watertight ?? null;
    const result: Record<string, unknown> = {
      sound,
      brep_valid: brepValid,
      watertight,
      manifold: cert?.manifold ?? null,
      self_intersection_free: cert?.self_intersection_free ?? null,
      construction_consistent: cert?.construction_consistent ?? null,
      labels_consistent: cert?.labels_consistent ?? null,
      tessellation_clean: cert?.tessellation_clean ?? null,
      mesh_quality_clean: cert?.mesh_quality_clean ?? null,
      euler_characteristic: cert?.euler_characteristic ?? null,
      open_edges: p?.open_edges ?? cert?.boundary_edges ?? null,
      nonmanifold_edges: p?.nonmanifold_edges ?? cert?.nonmanifold_edges ?? null,
      dims: p?.dims ?? null,
      face_count: part?.topology?.face_count ?? null,
      volume: part?.volume ?? null,
      errors: cert?.errors ?? null,
      // Full certificate breakdown present only on the ?full path (worst-face
      // pointers — the optimisation oracle).
      cert: cert ?? undefined,
      verdict:
        p?.verdict ??
        (sound
          ? "OK — valid closed solid (cheap verdict; verify_part to certify)"
          : "UNSOUND — see verify_part"),
    };
    // X-ray is OFF the ambient hot path (n³ SDF) — opt in with
    // ROSHERA_AMBIENT_PERCEPTION=xray, or use the explicit occupancy_view tool.
    if (process.env.ROSHERA_AMBIENT_PERCEPTION === "xray") {
      await addOccupancyXray(result, partId);
    }
    return result;
  } catch {
    return undefined;
  }
}

/**
 * Fetch a small shaded iso render as an MCP image content block — the FORM
 * channel of ambient perception. Same source `render_part` uses. Cheap (size
 * 320). Best-effort: returns `undefined` on any failure so the op's text result
 * still stands.
 */
async function ambientRender(partId: number): Promise<any | undefined> {
  try {
    const r = await api(
      "GET",
      `/api/agent/parts/${partId}/render?mode=shaded&view=iso&size=320`,
    );
    if (!r?.png_base64) return undefined;
    return { type: "image" as const, data: r.png_base64, mimeType: "image/png" };
  } catch {
    return undefined;
  }
}

/**
 * Project a full perception object onto ONE honest line — the TOKEN-DIET form
 * of ambient verification. The verdict is never dropped and never softened:
 * a sound part lists exactly the dimensions that were verified true; an
 * unsound part names every failed dimension loudly and points at verify_part
 * (full certificate + diagnostic render). Dimensions the hot path did not
 * compute (`null`) are reported as unverified, never fabricated.
 */
function compactVerdict(p: any): string {
  const DIMS: [string, string][] = [
    ["brep_valid", "brep"],
    ["watertight", "watertight"],
    ["manifold", "manifold"],
    ["self_intersection_free", "no-self-intersect"],
    ["tessellation_clean", "tess"],
    ["mesh_quality_clean", "mesh-quality"],
  ];
  const failed = DIMS.filter(([k]) => p?.[k] === false).map(([, n]) => n);
  const unverified = DIMS.filter(([k]) => p?.[k] == null).map(([, n]) => n);
  const facts: string[] = [];
  if (p?.euler_characteristic != null) facts.push(`χ=${p.euler_characteristic}`);
  if (typeof p?.volume === "number") facts.push(`vol=${p.volume.toFixed(1)}mm³`);
  if (p?.face_count != null) facts.push(`${p.face_count} faces`);
  if (p?.open_edges) facts.push(`⚠ ${p.open_edges} open edges`);
  if (p?.nonmanifold_edges) facts.push(`⚠ ${p.nonmanifold_edges} non-manifold edges`);
  if (p?.eyes_consistent === "inconsistent") failed.push("eyes-consistent");
  const tail = facts.length ? ` | ${facts.join(" | ")}` : "";
  if (p?.sound === true && failed.length === 0) {
    const verified = DIMS.filter(([k]) => p?.[k] === true).map(([, n]) => n);
    const suffix = unverified.length
      ? ` (unverified: ${unverified.join(",")} — verify_part to certify)`
      : "";
    return `SOUND ✓ ${verified.join("·")}${suffix}${tail}`;
  }
  const why = failed.length ? failed.join(", ") : "cheap verdict false";
  return `UNSOUND ✗ failed: ${why}${tail} — run verify_part for the full certificate + diagnostic render`;
}

/**
 * `ok()` plus AMBIENT PERCEPTION for the resulting part — every mutating op
 * carries its verdict with no extra tool call. Modes via
 * `ROSHERA_AMBIENT_PERCEPTION`:
 *  - `compact` (DEFAULT — the token diet): ONE honest verdict line
 *    (sound/unsound + verified/failed dimensions + χ/volume/faces). No image,
 *    no cert JSON. Depth on demand: verify_part (full certificate +
 *    diagnostic), render_part (form), ground_truth (provenance).
 *  - `full`: the legacy firehose — full perception object as text PLUS a
 *    shaded render image on every op.
 *  - `cert`: full perception object, no image.
 *  - `xray` (composes with the above fetch): adds the occupancy slice-stack.
 * `ROSHERA_MCP_AUTOVERIFY=0` is the master off switch (no perception at all).
 * Every channel degrades gracefully on fetch failure.
 */
async function okp(data: Record<string, unknown>, partId: number | null) {
  if (process.env.ROSHERA_MCP_AUTOVERIFY === "0") return ok(data);
  const perception = await perceive(partId);
  const mode = process.env.ROSHERA_AMBIENT_PERCEPTION ?? "compact";
  if (perception === undefined) return ok(data);
  if (mode === "compact" || mode === "") {
    // The token-diet default: one verdict line, no image, no cert JSON.
    return ok({ ...data, perception: compactVerdict(perception) });
  }
  // Legacy verbose modes keep their full behaviour: full/xray = perception
  // object + shaded render; cert = perception object only.
  const base = ok({ ...data, perception });
  if (partId === null || mode === "cert") {
    return base;
  }
  const image = await ambientRender(partId);
  if (image) base.content.push(image);
  return base;
}

// ─── Server + tools ────────────────────────────────────────────────────

// The Roshera mark (roshera-app/public/favicon.svg, inlined as a data URI so the
// server stays self-contained). MCP clients that render server icons (per the MCP
// `icons` spec, supported by SDK >= ~1.18) show this next to "Calling roshera";
// clients that don't yet render server icons simply show the name as text.
const ROSHERA_ICON =
  "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI0OCIgaGVpZ2h0PSI0OCIgdmlld0JveD0iMCAwIDQ4IDQ4Ij4NCiAgPGNpcmNsZSBjeD0iMjQiIGN5PSIyNCIgcj0iMjMiIGZpbGw9IiNGNURGQTAiIHN0cm9rZT0iI0Q0NjQ1QyIgc3Ryb2tlLXdpZHRoPSIyLjUiLz4NCiAgPHBhdGggZD0iTTE2IDM2VjEyaDhjNC40MTggMCA4IDMuMTM0IDggN3MtMy41ODIgNy04IDdoLTgiIGZpbGw9Im5vbmUiIHN0cm9rZT0iI0M0NTI0QSIgc3Ryb2tlLXdpZHRoPSIwIi8+DQogIDxwYXRoIGQ9Ik0xNiAxMiBMMjggMTIgUTMyIDEyIDMyIDE5IFEzMiAyNiAyOCAyNiBMMTYgMzYgWiIgZmlsbD0iI0M0NTI0QSIvPg0KICA8cGF0aCBkPSJNMjggMTIgUTMyIDEyIDMyIDE5IFEzMiAyNiAyOCAyNiBaIiBmaWxsPSIjRDQ2NDVDIi8+DQo8L3N2Zz4NCg==";

const server = new McpServer({
  // Display name Claude Code shows in "Calling …". The 🅡 glyph (negative
  // circled R) renders the Roshera mark inline in the CLI status line.
  name: "🅡 ROSHERA",
  version: "0.1.0",
  icons: [{ src: ROSHERA_ICON, mimeType: "image/svg+xml", sizes: ["48x48"] }],
});

// ---- perception -------------------------------------------------------

server.tool(
  "render_part",
  "SEE a part: deterministic offscreen render returned as an image. " +
    "mode 'shaded' shows form; 'ids' paints every B-Rep face a distinct " +
    "flat color and returns a color→face_id legend (use it to ADDRESS " +
    "topology: 'the red face is face 12'); 'depth' and 'normals' are exact " +
    "G-buffer channels.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    mode: z
      .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
      .default("shaded"),
    view: z.enum(["iso", "front", "top", "right"]).default("iso"),
    size: z.number().int().min(64).max(2048).default(512),
  },
  async ({ part_id, mode, view, size }) => {
    try {
      const r = await api(
        "GET",
        `/api/agent/parts/${part_id}/render?mode=${mode}&view=${view}&size=${size}`,
      );
      const content: any[] = [
        { type: "image", data: r.png_base64, mimeType: "image/png" },
      ];
      if (mode === "ids") {
        content.push({
          type: "text",
          text: `face legend (rgb → face_id): ${JSON.stringify(r.face_legend)}`,
        });
      }
      if (mode === "diagnostic") {
        content.push({
          type: "text",
          text:
            `defects — open_edges (red hole rims, missing faces): ${r.open_edges}; ` +
            `nonmanifold_edges (magenta, overlapping faces): ${r.nonmanifold_edges}. ` +
            `Both 0 = watertight. Front-face culled, so missing/flipped faces read as see-through holes.`,
        });
      }
      return { content };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "ground_truth",
  "ASK THE KERNEL what a part actually is and whether it's real — the kernel's " +
    "OWN account, not a guess. Returns PROVENANCE (which operation made it: a " +
    "designed surface — nurbs_loft / revolve / extrude / loft / sweep / boolean / " +
    "fillet / chamfer — vs a bare PRIMITIVE stand-in like a box/cylinder) and the " +
    "kernel-COMPUTED validity certificate (brep_valid, watertight, manifold, " +
    "euler, sound + a one-line summary). ALSO returns `tessellation` — the DISPLAY-" +
    "MESH verdict: `tessellation_clean:false` means the render mesh is degenerate " +
    "or has inverted normals (e.g. a bored cylinder's inner wall tessellating to a " +
    "scribble) even when the B-Rep is watertight; `tessellation.worst_face` names " +
    "the exact face so you can SEE the defect without rendering. The kernel cannot " +
    "misrepresent this, so use it to confirm a build is a genuine closed DESIGNED " +
    "solid that also RENDERS correctly before trusting or reporting it — " +
    "`designed:false`/`sound:false` (now incl. a bad display mesh) means stop and fix, not ship.",
  { part_id: z.number().int().describe("kernel part id from list_parts") },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/truth`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "assembly_verify",
  "CERTIFY A KINEMATIC ASSEMBLY — the non-fakeable 'does this physically go " +
    "together AND move without collision?' verdict over LIVE kernel parts. You " +
    "declare how parts MATE (which axes are concentric, which faces are " +
    "coincident) and which are MECHANISMS (a gimbal, an actuator); the kernel " +
    "tessellates each part, solves the mate constraints, and returns a 5-dimension " +
    "certificate: `mates_consistent` (the constraint system is satisfiable), " +
    "`fully_grounded` (every part reaches the ground part through a mate path — a " +
    "part with NO mate to ground is FLOATING and named in `floating_instances`), " +
    "`dof`+`mobility` (residual freedom), `no_static_interference` (no two parts " +
    "overlap at the solved pose), `swept_clearance_ok` (every mechanism stays " +
    "clear across its FULL range of motion — Parry CCD, made conservative by " +
    "`epsilon`). `is_sound` is the AND of all five. USE THIS to prove an assembly " +
    "is not a floating massing: a part placed by coordinate with no mate is caught " +
    "here even when a shaded render and a per-part 'SOUND' verdict cannot see it.\n\n" +
    "MATE format (get face geometry from plane_from_face; an axis is the part's " +
    "build axis): { \"kind\":\"Concentric\"|\"Coincident\"|\"Fixed\", \"a\":<instance_id>, " +
    "\"feature_a\":{\"Axis\":{\"origin\":[x,y,z],\"direction\":[x,y,z]}} OR " +
    "{\"Face\":{\"point\":[x,y,z],\"normal\":[x,y,z]}}, \"b\":<instance_id>, \"feature_b\":{...} }. " +
    "Concentric takes two Axis features; Coincident two Face features (mating faces " +
    "have ANTIPARALLEL normals).\n" +
    "MECHANISM format: { \"moving\":<instance_id>, \"joint\":" +
    "{\"Revolute\":{\"axis_origin\":[..],\"axis_dir\":[..]}} OR {\"Prismatic\":{\"axis_origin\":[..]," +
    "\"axis_dir\":[..]}} OR {\"Spherical\":{\"center\":[..]}} OR \"Fixed\", " +
    "\"base_translation\":[x,y,z], \"base_rotation\":[x,y,z,w], \"range\":[lo,hi], \"samples\":<n> }.",
  {
    ground: z
      .number()
      .int()
      .describe("instance_id of the grounded (fixed reference) part"),
    parts: z
      .array(
        z.object({
          object: z.string().describe("the part's object_uuid"),
          instance_id: z
            .number()
            .int()
            .describe("this occurrence's id, referenced by mates/mechanisms"),
          translation: z
            .array(z.number())
            .length(3)
            .optional()
            .describe("world position [x,y,z] (default origin)"),
          rotation: z
            .array(z.number())
            .length(4)
            .optional()
            .describe("unit quaternion [x,y,z,w] (default identity)"),
        }),
      )
      .describe("every part in the assembly, as an instance"),
    mates: z
      .array(z.any())
      .optional()
      .describe("mate constraints — see the MATE format in the description"),
    mechanisms: z
      .array(z.any())
      .optional()
      .describe("mechanisms for swept-clearance — see the MECHANISM format"),
    epsilon: z
      .number()
      .optional()
      .default(0.0)
      .describe(
        "tessellation deviation bound; certified clearance = parry_distance − epsilon (conservative)",
      ),
  },
  async ({ ground, parts, mates, mechanisms, epsilon }) => {
    try {
      const body = {
        ground,
        parts,
        mates: mates ?? [],
        mechanisms: mechanisms ?? [],
        epsilon: epsilon ?? 0.0,
      };
      return ok(await api("POST", "/api/assembly/verify", body));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "occupancy_view",
  "SEE a part's STRUCTURE as a non-deceivable SDF X-ray — a coarse occupancy " +
    "slice-stack ('#'=solid, '.'=empty) that reveals internal cavities, wall " +
    "thickness, gaps and through-holes a shaded render can hide. Complements " +
    "render_part (form) and ground_truth (validity). The kernel samples its EXACT " +
    "signed-distance field on an n×n×n grid over the part's bbox; each layer is a " +
    "z-slice (header 'z=k', then n rows of n chars, rows by y, cols by x). Larger " +
    "n is a sharper X-ray (capped at 48).",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    n: z
      .number()
      .int()
      .optional()
      .default(20)
      .describe("grid resolution per axis (clamped to 4..48)"),
  },
  async ({ part_id, n }) => {
    try {
      const r = await api("GET", `/api/agent/parts/${part_id}/occupancy?n=${n}`);
      const summary = `occupancy n=${r.n} dims=${JSON.stringify(r.dims)} fill_fraction=${r.fill_fraction.toFixed(3)} bbox=${JSON.stringify(r.bbox)}`;
      return {
        content: [
          { type: "text" as const, text: summary },
          { type: "text" as const, text: r.slices },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "scene_view",
  "SEE THE WHOLE SCENE: composite EVERY part into one image from an arbitrary " +
    "camera (azimuth/elevation, world-Z up), auto-framed to the combined bounds. " +
    "The way to look at a multi-part ASSEMBLY (a car, a mechanism) as a whole " +
    "instead of one part at a time — change az/el to orbit and inspect from any " +
    "angle. `mode:'diagnostic'` highlights open (red) / non-manifold (magenta) " +
    "edges across the assembly.",
  {
    az: z.number().default(35).describe("azimuth degrees around world Z"),
    el: z.number().default(20).describe("elevation degrees above the horizon"),
    mode: z
      .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
      .default("shaded"),
    size: z.number().int().min(64).max(2048).default(720),
    quality: z
      .enum(["coarse", "medium", "fine"])
      .default("medium")
      .describe("mesh tessellation quality: coarse=fast, fine=resolve curved silhouettes"),
  },
  async ({ az, el, mode, size, quality }) => {
    try {
      const r = await api(
        "GET",
        `/api/agent/scene/orbit?az=${az}&el=${el}&mode=${mode}&size=${size}&quality=${quality}`,
      );
      return {
        content: [
          { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
          {
            type: "text" as const,
            text: `scene az=${az}° el=${el}° dir=${JSON.stringify(r.dir)} open=${r.open_edges} nm=${r.nonmanifold_edges}`,
          },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "set_part_color",
  "Colour a part for the scene-eye: set its display RGB so `scene_view` renders " +
    "it in that colour (black tyres, livery body, …). Registry-only — does NOT " +
    "modify geometry. After colouring parts, call `scene_view` to see the result.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    r: z.number().int().min(0).max(255),
    g: z.number().int().min(0).max(255),
    b: z.number().int().min(0).max(255),
  },
  async ({ part_id, r, g, b }) => {
    try {
      const res = await api("POST", `/api/agent/parts/${part_id}/color`, { r, g, b });
      return ok(res);
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "select_face",
  "Address a face by DESCRIPTION instead of guessing its id — the kernel " +
    "resolves it or REFUSES. Give a surface `kind` (planar/cylindrical/…), an " +
    "optional `normal_dir` the face must point along (e.g. [0,0,1] for the top), " +
    "and an optional `extremal` tie-breaker (largest_area/smallest_area/" +
    "most_along). Returns the concrete face_id + its persistent_id (stable across " +
    "edits) on success; on `ambiguous` it returns the candidate face_ids (refine " +
    "the description) and on `not_found` nothing matched. The kernel NEVER picks " +
    "one of several equal matches for you — that's the point.",
  {
    part_id: z.number().int(),
    kind: z
      .enum(["any", "planar", "cylindrical", "spherical", "conical", "toroidal", "nurbs"])
      .default("any"),
    normal_dir: z.tuple([z.number(), z.number(), z.number()]).optional(),
    extremal: z
      .enum(["none", "largest_area", "smallest_area", "most_along"])
      .default("none"),
    along: z.tuple([z.number(), z.number(), z.number()]).optional(),
    angle_tol_deg: z.number().default(12),
  },
  async ({ part_id, kind, normal_dir, extremal, along, angle_tol_deg }) => {
    try {
      // Read the body regardless of status: 404 (not_found) / 409 (ambiguous)
      // are the kernel's meaningful REFUSALS, not transport errors.
      const res = await fetch(`${BASE}/api/agent/parts/${part_id}/select-face`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          kind,
          normal_dir: normal_dir ?? null,
          extremal,
          along: along ?? null,
          angle_tol_deg,
        }),
      });
      const j = await res.json();
      return ok(j);
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "select_edge",
  "Address an EDGE by description — the kernel resolves it or REFUSES. Give a " +
    "`curve_kind` (line/arc/circle/nurbs), a `blend` filter (filleted/chamfered/" +
    "unblended — so 'the fillet edge' resolves), an optional `direction` the edge " +
    "must run along, and an optional `extremal` (longest/shortest/most_along). " +
    "Returns edge_id + persistent_id, or `ambiguous` (candidate edge_ids) / " +
    "`not_found`. The kernel never picks among equal matches for you.",
  {
    part_id: z.number().int(),
    curve_kind: z.enum(["any", "line", "arc", "circle", "nurbs"]).default("any"),
    blend: z.enum(["any", "filleted", "chamfered", "unblended"]).default("any"),
    direction: z.tuple([z.number(), z.number(), z.number()]).optional(),
    extremal: z.enum(["none", "longest", "shortest", "most_along"]).default("none"),
    along: z.tuple([z.number(), z.number(), z.number()]).optional(),
    angle_tol_deg: z.number().default(12),
  },
  async ({ part_id, curve_kind, blend, direction, extremal, along, angle_tol_deg }) => {
    try {
      const res = await fetch(`${BASE}/api/agent/parts/${part_id}/select-edge`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          curve_kind,
          blend,
          direction: direction ?? null,
          extremal,
          along: along ?? null,
          angle_tol_deg,
        }),
      });
      return ok(await res.json());
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "verify_part",
  "SELF-CHECK a part's geometry — the EXPLICIT FULL CERTIFICATE (the expensive " +
    "checks the sub-second ambient verdict deliberately skips). Runs the kernel's " +
    "complete certificate (`?full=1`): brep_valid + watertight + manifold + " +
    "self-intersection-free + construction-consistency + tessellation- and " +
    "mesh-quality — the authoritative, non-fakeable 'is this a real, closed, " +
    "non-self-overlapping solid' answer. Display-mesh open/non-manifold edge " +
    "counts are reported separately as render quality (a valid B-Rep can still " +
    "tessellate with T-junctions — KNOWN_BUGS #65 — which is not a defect). " +
    "ALWAYS call this after a boolean or multi-feature build. Returns the iso " +
    "diagnostic image so you can SEE where any real (red=open / magenta=" +
    "non-manifold) defect is.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    view: z.enum(["iso", "front", "top", "right"]).default("iso"),
  },
  async ({ part_id, view }) => {
    try {
      // EXPLICIT FULL CERT: ?full=1 runs the complete (O(n²)) certificate. This
      // is the verify path, so the full budget (TIMEOUT_MS) applies — we WANT the
      // expensive checks here. Image + display-mesh counts from the render.
      const p = await api("GET", `/api/agent/parts/${part_id}/perception?full=1`);
      const r = await api(
        "GET",
        `/api/agent/parts/${part_id}/render?mode=diagnostic&view=${view}&size=720`,
      );
      const cert = p?.cert ?? null;
      const valid = (cert?.brep_valid ?? p.valid) === true;
      const sound = p?.sound === true;
      const meshClean = r.open_edges === 0 && r.nonmanifold_edges === 0;
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(
              {
                part_id,
                sound,
                brep_valid: valid,
                brep_watertight: (cert?.watertight ?? p.watertight) === true,
                manifold: cert?.manifold ?? null,
                self_intersection_free: cert?.self_intersection_free ?? null,
                tessellation_clean: cert?.tessellation_clean ?? null,
                mesh_quality_clean: cert?.mesh_quality_clean ?? null,
                construction_consistent: cert?.construction_consistent ?? null,
                // Dual-eye gate: "consistent" | "inconsistent" | "not_applicable".
                // Feeds `sound` — an inconsistent dual-eye means the B-Rep cert and the
                // mesh cert disagree; the solid is flagged UNSOUND.
                eyes_consistent: cert?.eyes_consistent ?? "not_applicable",
                verdict: p?.verdict ??
                  (!valid
                    ? "BROKEN — B-Rep invalid (a real topological defect; see the image)"
                    : meshClean
                      ? "OK — valid closed solid"
                      : "OK — valid solid; display mesh has tessellation T-junctions only (not a defect)"),
                display_mesh: {
                  open_edges: r.open_edges,
                  nonmanifold_edges: r.nonmanifold_edges,
                  note: "display tessellation quality only — does NOT determine validity",
                },
                dims: p.dims ?? null,
                // Advisory dual-eye reconcile report. {"status":"pending"} when the async
                // worker has not yet completed for the current solid state. When ready:
                // {status, discrepancies:[{severity, kind, description}], coverage:{seen,total}}.
                reconcile: p?.reconcile ?? { status: "pending" },
                cert: cert ?? undefined,
              },
              null,
              2,
            ),
          },
          { type: "image", data: r.png_base64, mimeType: "image/png" },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "make_drawing",
  "Generate a 2D engineering DRAWING from a part: the standard four-view " +
    "sheet — Front / Top / Right plus an isometric pictorial — with " +
    "hidden-line removal, centerlines, and automatic dimensions. The sheet " +
    "size and scale are chosen to fit the part (small parts on A4, growing " +
    "to A0), and the views are centered with proper offset dimension lines. " +
    "Returns the new drawing id (open it in the Drawing workspace, or fetch " +
    "GET /api/drawings/<id>/svg|pdf|dxf) AND a QUALITY report — the 2D " +
    "perception layer: whether the layout passed, sheet utilization, and any " +
    "issues (views overlapping, off-sheet, dimensions on the outline). Treat " +
    "the quality report the way you treat watertightness for 3D geometry.",
  {
    part_id: z.number().int().describe("kernel part/solid id from list_parts"),
    name: z.string().optional().describe("title-block name for the sheet"),
  },
  async ({ part_id, name }) => {
    try {
      const qs = name ? `?name=${encodeURIComponent(name)}` : "";
      const r = await api("POST", `/api/parts/${part_id}/drawing${qs}`);
      const q = r?.quality ?? null;
      return ok({
        drawing_id: r?.id ?? null,
        quality: q,
        verdict: q
          ? q.passed
            ? `OK — clean sheet (${Math.round((q.sheet_utilization ?? 0) * 100)}% utilization, ${
                q.issues?.length ?? 0
              } advisory issue(s))`
            : `LAYOUT ISSUES — ${q.issues?.length ?? 0} finding(s); see quality.issues`
          : "drawing created (no quality report)",
        note: "Open in the Drawing workspace, or fetch_drawing to save PDF/DXF/SVG to disk.",
      });
    } catch (e) {
      return fail(e);
    }
  },
);

/** Save raw bytes fetched from a backend path to an absolute file on disk. */
async function saveBinary(urlPath: string, savePath: string): Promise<number> {
  const res = await fetch(`${BASE}${urlPath}`);
  if (!res.ok) {
    throw new Error(`GET ${urlPath} → ${res.status}`);
  }
  const buf = Buffer.from(await res.arrayBuffer());
  const { writeFile, mkdir } = await import("node:fs/promises");
  const { dirname } = await import("node:path");
  await mkdir(dirname(savePath), { recursive: true });
  await writeFile(savePath, buf);
  return buf.length;
}

/** Default save directory: ~/Desktop (falls back to the home dir). */
async function defaultSaveDir(): Promise<string> {
  const { homedir } = await import("node:os");
  const { join } = await import("node:path");
  return join(homedir(), "Desktop");
}

server.tool(
  "export_part",
  "EXPORT one or more parts to a real CAD file on disk — STEP (AP242, mm), " +
    "STL, or OBJ — and return the absolute file path. This is the " +
    "production hand-off: the STEP opens in FreeCAD/SolidWorks/slicers. " +
    "`objects` are object_uuids (empty = every solid in the scene). Saves " +
    "to `save_path` if given, else ~/Desktop/<file_name>.",
  {
    format: z.enum(["STEP", "STL", "OBJ"]).default("STEP"),
    objects: z.array(z.string().uuid()).default([]),
    file_name: z
      .string()
      .regex(/^[\w.-]+$/)
      .describe("file name without directory, e.g. flange_2in.step"),
    save_path: z
      .string()
      .optional()
      .describe("absolute destination path; overrides file_name/Desktop"),
    quality: z.enum(["Low", "Medium", "High"]).default("High"),
  },
  async ({ format, objects, file_name, save_path, quality }) => {
    try {
      const r = await api("POST", "/api/export", { format, objects, quality });
      if (!r?.download_url) {
        throw new Error(`export returned no download_url: ${JSON.stringify(r)}`);
      }
      const { join } = await import("node:path");
      const dest = save_path ?? join(await defaultSaveDir(), file_name);
      const bytes = await saveBinary(r.download_url, dest);
      return ok({
        saved_to: dest,
        bytes,
        format,
        parts_exported: objects.length === 0 ? "all" : objects.length,
        export_time_ms: r.export_time_ms ?? null,
      });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "fetch_drawing",
  "SAVE a drawing produced by make_drawing to disk as PDF, DXF, or SVG and " +
    "return the absolute file path — the shop-ready sheet without leaving " +
    "the conversation.",
  {
    drawing_id: z.string().uuid().describe("drawing_id returned by make_drawing"),
    format: z.enum(["pdf", "dxf", "svg"]).default("pdf"),
    file_name: z
      .string()
      .regex(/^[\w.-]+$/)
      .describe("file name without directory, e.g. flange_drawing.pdf"),
    save_path: z
      .string()
      .optional()
      .describe("absolute destination path; overrides file_name/Desktop"),
  },
  async ({ drawing_id, format, file_name, save_path }) => {
    try {
      const { join } = await import("node:path");
      const dest = save_path ?? join(await defaultSaveDir(), file_name);
      const bytes = await saveBinary(`/api/drawings/${drawing_id}/${format}`, dest);
      return ok({ saved_to: dest, bytes, format });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "get_pointer",
  "What is the HUMAN pointing at in the viewport right now? Returns their " +
    "latest click (object, face_id, world position) joined with the " +
    "kernel's hover report (surface type, area, host part). Use to ground " +
    "'this face / this hole / here'.",
  {},
  async () => {
    try {
      return ok(await api("GET", "/api/agent/pointer"));
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        return ok({ pointer: null, note: "user has not clicked anything yet" });
      }
      return fail(e);
    }
  },
);

// ---- inspection -------------------------------------------------------

server.tool(
  "list_parts",
  "List every part in the live model (id, name, kind).",
  {},
  async () => {
    try {
      return ok(await api("GET", "/api/agent/parts"));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "get_part",
  "Full report for one part: world placement (center, dimensions, anchor " +
    "datum), topology fingerprint, name.",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "mass_properties",
  "Exact mass properties: volume, mass, center of mass, inertia tensor, " +
    "principal axes (mesh-integrated, accuracy-gated against closed form).",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/mass`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "verify_claim",
  "VERIFY a math claim against the kernel's GROUND TRUTH — 'the notebook that can't " +
    "lie'. Bind each variable in `expr` to an EXACT kernel measurement (a part's " +
    "volume / surface_area, a face's area, an edge's length, or a supplied constant), " +
    "assert `expected`, and the kernel evaluates it deterministically (NOT an LLM). " +
    "Returns a THREE-STATE verdict: verified | false (a real mismatch, with abs_error) " +
    "| refused (a binding could not resolve — never a silent pass). Use it to check an " +
    "engineering relation against measured geometry, e.g. nozzle expansion ratio " +
    "A_exit/A_throat, or mass = volume * density.",
  {
    expr: z
      .string()
      .describe("math expression over the binding variable names, e.g. 'a_exit / a_throat'"),
    bindings: z
      .array(
        z.object({
          var: z.string().describe("variable name used in expr"),
          measure: z.object({
            kind: z.enum([
              "volume",
              "surface_area",
              "face_area",
              "edge_length",
              "constant",
            ]),
            part: z
              .string()
              .optional()
              .describe("part object UUID — for volume / surface_area"),
            face: z.number().int().optional().describe("face id — for face_area"),
            edge: z.number().int().optional().describe("edge id — for edge_length"),
            value: z.number().optional().describe("the value — for constant"),
          }),
        }),
      )
      .describe("variable→measurement bindings"),
    expected: z.number().describe("the value the expression should equal"),
    tolerance: z
      .number()
      .optional()
      .describe("absolute tolerance; omit for auto (1e-6 relative)"),
  },
  async ({ expr, bindings, expected, tolerance }) => {
    try {
      return ok(
        await api("POST", "/api/agent/verify-claim", {
          expr,
          bindings,
          expected,
          ...(tolerance !== undefined ? { tolerance } : {}),
        }),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "get_revolve_profile",
  "RECOVER the editable meridian a revolved part was built from — the (r, z) " +
    "half-plane profile (radius-from-axis, height-along-axis). This is the " +
    "scientist's editable source: read it, change a radius (e.g. widen the throat), " +
    "and call revolve again with the edited profile to REGENERATE the part (the " +
    "edit→regenerate loop). Returns [[r,z],...]; 404 if the part was not built by a " +
    "parametric revolve.",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/profile`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "get_face",
  "Per-face report: surface type, area, principal curvatures " +
    "([0,0]=flat, [±1/r,0]=cylindrical, [±1/r,±1/r]=spherical), boundary " +
    "edges, neighbours. Face ids come from render_part mode 'ids' or " +
    "get_pointer.",
  { face_id: z.number().int() },
  async ({ face_id }) => {
    try {
      return ok(await api("GET", `/api/agent/faces/${face_id}`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "part_distance",
  "MEASURE the spatial relationship between two parts from their world AABBs: " +
    "the gap (clearance when separated), the overlap (penetration when they " +
    "interpenetrate), the center-to-center distance, and the unit DIRECTION from " +
    "a to b. Use it to check clearance, confirm a mate seats, or decide which way " +
    "to nudge a part. Both ids are kernel part ids from list_parts.",
  {
    part_a: z.number().int().describe("kernel part id of the first part"),
    part_b: z.number().int().describe("kernel part id of the second part"),
  },
  async ({ part_a, part_b }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/distance/${part_a}/${part_b}`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "part_features",
  "READ a part's analytic FEATURE sizes straight off the B-Rep: every face's " +
    "feature dimension (cylinder diameters + axes for bores/bosses, plane normals) " +
    "PLUS a distinct-diameter summary (each unique bore/boss diameter and how many " +
    "occur). The way to ask 'what holes and bosses does this have and how big' " +
    "without measuring pixels — values are exact, read off the analytic surfaces.",
  { part_id: z.number().int().describe("kernel part id from list_parts") },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/features`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "part_coverage",
  "COVERAGE honesty (EYE-5): report which faces the 4 standard views (front / " +
    "top / right / iso) actually SHOW versus leave unseen, so you know when a face " +
    "is hidden and you must request another camera angle (scene_view with a custom " +
    "az/el) instead of assuming you saw the whole part. Returns seen vs unseen face " +
    "ids and the per-view visibility.",
  { part_id: z.number().int().describe("kernel part id from list_parts") },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/coverage`));
    } catch (e) {
      return fail(e);
    }
  },
);

// ---- spatial primitives: point / ray / region ------------------------
// SDF-verified kernel queries (point-in-solid + nearest, analytic ray-cast,
// region intersection). The agent's continuous handle on the solid — probe a
// point, shoot a ray, ask what is inside a box/sphere — all exact-analytic.

server.tool(
  "point_query",
  "PROBE a world point against a part: returns the SIGNED DISTANCE to the solid " +
    "(negative INSIDE the material, positive OUTSIDE, ~0 on the surface), the " +
    "inside/outside/on classification, and the NEAREST boundary face id + the exact " +
    "closest point on it. Exact-analytic (ray-parity sign + analytic nearest), never " +
    "a mesh lookup — a point in a through-hole reads OUTSIDE because the hole is not " +
    "material. Use it for clearance probes, to test if a coordinate is inside a part, " +
    "or to find the nearest surface to a location.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    point: z
      .tuple([z.number(), z.number(), z.number()])
      .describe("world-space query point [x, y, z]"),
  },
  async ({ part_id, point }) => {
    try {
      return ok(
        await api("POST", `/api/agent/parts/${part_id}/point-query`, { point }),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "ray_query",
  "CAST a ray through a part and get the ORDERED list of face crossings along it " +
    "(face id, exact world hit point, oriented surface normal, distance from the " +
    "origin), sorted near→far. Exact analytic surface intersections clipped to each " +
    "face's real trim loops — never a mesh approximation; a missing face renders as " +
    "see-through (no phantom hit). Use it to find what a sightline hits, measure a " +
    "wall's two crossings (thickness), or locate a feature along a direction. An " +
    "empty hits list means the ray missed.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    origin: z
      .tuple([z.number(), z.number(), z.number()])
      .describe("ray origin [x, y, z]"),
    direction: z
      .tuple([z.number(), z.number(), z.number()])
      .describe("ray direction [x, y, z] (need not be unit length)"),
  },
  async ({ part_id, origin, direction }) => {
    try {
      return ok(
        await api("POST", `/api/agent/parts/${part_id}/ray-query`, {
          origin,
          direction,
        }),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "region_query",
  "ASK 'what is in here?': given an axis-aligned BOX (center + half_extents) OR a " +
    "SPHERE (center + radius), return which parts and which of their face ids meet " +
    "the volume, and whether the region is EMPTY. Pass `part_id` to query one part, " +
    "or omit it to scan the WHOLE scene (e.g. 'is anything inside this clearance " +
    "envelope?'). Face extents are exact trim-curve envelopes, not the mesh. Give " +
    "either half_extents (box) or radius (sphere); box wins if both are supplied.",
  {
    center: z
      .tuple([z.number(), z.number(), z.number()])
      .describe("region center [x, y, z]"),
    half_extents: z
      .tuple([z.number(), z.number(), z.number()])
      .optional()
      .describe("box half-extents [hx, hy, hz] — supply for a BOX region"),
    radius: z
      .number()
      .positive()
      .optional()
      .describe("sphere radius — supply for a SPHERE region (ignored if half_extents given)"),
    part_id: z
      .number()
      .int()
      .optional()
      .describe("restrict to one part; omit to scan every part in the scene"),
  },
  async ({ center, half_extents, radius, part_id }) => {
    try {
      if (!half_extents && radius === undefined) {
        return fail(new Error("provide half_extents (box) or radius (sphere)"));
      }
      return ok(
        await api("POST", "/api/agent/region-query", {
          center,
          ...(half_extents ? { half_extents } : {}),
          ...(radius !== undefined ? { radius } : {}),
          ...(part_id !== undefined ? { part_id } : {}),
        }),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

// ---- mutation: deletion ----------------------------------------------

server.tool(
  "delete_part",
  "Delete one part (timeline-recorded, undo-safe). WARNING: kernel part " +
    "ids RENUMBER after deletion — re-run list_parts before further " +
    "deletes; never reuse stale ids.",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("DELETE", `/api/agent/parts/${part_id}`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "clear_parts",
  "Delete EVERY part (each deletion timeline-recorded, undo-safe). Safe to " +
    "build immediately after (the post-clear extrude wedge, bug #26, is fixed).",
  {},
  async () => {
    try {
      return ok(await api("DELETE", "/api/agent/parts"));
    } catch (e) {
      return fail(e);
    }
  },
);

// ---- mutation: sketch & build ----------------------------------------

const PlaneSchema = z
  .union([
    z.enum(["xy", "xz", "yz"]),
    z.object({
      origin: z.tuple([z.number(), z.number(), z.number()]),
      u_axis: z.tuple([z.number(), z.number(), z.number()]),
      v_axis: z.tuple([z.number(), z.number(), z.number()]),
    }),
  ])
  .describe(
    "'xy' | 'xz' | 'yz' or a custom plane {origin, u_axis, v_axis} (e.g. " +
      "from plane_from_face)",
  );

server.tool(
  "create_sketch",
  "Start a sketch session on a plane. Returns sketch_id for the shape/" +
    "point/extrude tools. Prefer the composite tools (create_box / " +
    "create_cylinder / sketch_polygon) when they fit — fewer round trips.",
  {
    plane: PlaneSchema,
    tool: z.enum(["rectangle", "circle", "polyline"]),
  },
  async ({ plane, tool }) => {
    try {
      const s = await api("POST", "/api/sketch", { plane, tool });
      return ok({ sketch_id: s.id, shapes: s.shapes?.length ?? 1 });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "sketch_add_shape",
  "Add another shape to an existing sketch (e.g. hole circles inside an " +
    "outer boundary — region detection assigns outer/hole roles at " +
    "extrude time). Returns the new shape_index.",
  {
    sketch_id: z.string(),
    tool: z.enum(["rectangle", "circle", "polyline"]),
  },
  async ({ sketch_id, tool }) => {
    try {
      const s = await api("POST", `/api/sketch/${sketch_id}/shape`, { tool });
      return ok({ shape_index: (s.shapes?.length ?? 1) - 1 });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "sketch_points",
  "BATCH-add points to a sketch shape in one call (rectangle: 2 corners; " +
    "circle: center then a point on the radius; polyline: every vertex of " +
    "the closed polygon — a 96-point gear outline is ONE call). " +
    "shape_index omitted = the first shape.",
  {
    sketch_id: z.string(),
    points: z.array(z.tuple([z.number(), z.number()])).min(1),
    shape_index: z.number().int().optional(),
  },
  async ({ sketch_id, points, shape_index }) => {
    try {
      const base =
        shape_index === undefined
          ? `/api/sketch/${sketch_id}/point`
          : `/api/sketch/${sketch_id}/shape/${shape_index}/point`;
      for (const p of points) {
        await api("POST", base, { point: p });
      }
      return ok({ added: points.length });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "sketch_extrude",
  "Extrude the sketch into a solid. Multi-shape sketches get region " +
    "detection (outer boundary + holes). Returns the new part id and its " +
    "world placement.",
  {
    sketch_id: z.string(),
    distance: z.number(),
    name: z.string().optional(),
  },
  async ({ sketch_id, distance, name }) => {
    try {
      await api("POST", `/api/sketch/${sketch_id}/extrude`, {
        distance,
        name: name ?? null,
      });
      const id = await newestPartId();
      return await okp({ part_id: id, placement: id !== null ? await placement(id) : null }, id);
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "plane_from_face",
  "Derive a sketch plane FROM an existing planar face (stack features on " +
    "decks: 'sketch on this face'). object_id is the part's public UUID; " +
    "face_id from get_pointer or render legend. Returns {origin, u_axis, " +
    "v_axis} to pass as create_sketch's plane.",
  { object_id: z.string().uuid(), face_id: z.number().int() },
  async ({ object_id, face_id }) => {
    try {
      return ok(
        await api("POST", "/api/sketch/plane-from-face", { object_id, face_id }),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

// ---- composite creators (one call = one part) -------------------------

server.tool(
  "create_box",
  "ONE-CALL analytic box: width × depth centered at (cx, cy) on the chosen " +
    "plane, extruded by height along the plane normal. Uses the analytic box " +
    "primitive (canonical, all-Forward faces) — NOT sketch+extrude, which left a " +
    "non-canonical bottom cap that broke unions (two boxes → non-manifold). " +
    "Returns part id + world placement.",
  {
    plane: PlaneSchema.default("xy"),
    cx: z.number().default(0),
    cy: z.number().default(0),
    width: z.number().positive(),
    depth: z.number().positive(),
    height: z.number(),
    name: z.string().optional(),
  },
  async ({ plane, cx, cy, width, depth, height, name }) => {
    try {
      // Resolve the plane to a world origin + in-plane (u, v) basis (same as
      // create_cylinder). The box base centre = origin + cx·u + cy·v; the
      // analytic /api/geometry/box endpoint extrudes it by `height` along u×v.
      const std: Record<string, { o: number[]; u: number[]; v: number[] }> = {
        xy: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 1, 0] },
        xz: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 0, 1] },
        yz: { o: [0, 0, 0], u: [0, 1, 0], v: [0, 0, 1] },
      };
      const p =
        typeof plane === "string"
          ? std[plane]
          : { o: plane.origin, u: plane.u_axis, v: plane.v_axis };
      const center = [0, 1, 2].map((i) => p.o[i] + cx * p.u[i] + cy * p.v[i]);
      const r = await api("POST", "/api/geometry/box", {
        center,
        u_axis: p.u,
        v_axis: p.v,
        width,
        depth,
        height,
        name: name ?? null,
      });
      const id = await newestPartId();
      return await okp(
        {
          object_uuid: r.object?.id ?? null, // operand for boolean / transform
          part_id: id,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "create_cylinder",
  "ONE-CALL analytic cylinder: radius at (cx, cy) on the chosen plane, extruded " +
    "by height along the plane normal. Uses the analytic cylinder primitive " +
    "(one smooth lateral face) — NOT sketch+extrude, which produced an " +
    "inside-out lateral mesh (⅓ volume, negative inertia, dropped boss walls in " +
    "coaxial bores).",
  {
    plane: PlaneSchema.default("xy"),
    cx: z.number().default(0),
    cy: z.number().default(0),
    radius: z.number().positive(),
    height: z.number(),
    name: z.string().optional(),
  },
  async ({ plane, cx, cy, radius, height, name }) => {
    try {
      // Resolve the plane to a world origin + in-plane (u, v) basis.
      const std: Record<string, { o: number[]; u: number[]; v: number[] }> = {
        xy: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 1, 0] },
        xz: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 0, 1] },
        yz: { o: [0, 0, 0], u: [0, 1, 0], v: [0, 0, 1] },
      };
      const p =
        typeof plane === "string"
          ? std[plane]
          : { o: plane.origin, u: plane.u_axis, v: plane.v_axis };
      const cross = (a: number[], b: number[]) => [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
      ];
      // Cylinder base centre = plane origin + cx·u + cy·v; axis = u × v (the
      // plane normal), matching the old sketch-extrude placement.
      const center = [0, 1, 2].map(
        (i) => p.o[i] + cx * p.u[i] + cy * p.v[i],
      );
      const axis = cross(p.u, p.v);
      const r = await api("POST", "/api/geometry/cylinder", {
        center,
        axis,
        radius,
        height,
        name: name ?? null,
      });
      const id = await newestPartId();
      return await okp(
        {
          object_uuid: r.object?.id ?? null, // operand for boolean / transform
          part_id: id,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "create_cone",
  "ONE-CALL analytic cone or frustum. base_radius is the radius at `center`; " +
    "top_radius (default 0 = sharp apex) is the radius at center+axis*height. " +
    "A true smooth cone surface (not faceted). Returns part id + placement.",
  {
    center: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 0]),
    axis: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 1]),
    base_radius: z.number().nonnegative(),
    top_radius: z.number().nonnegative().default(0),
    height: z.number().positive(),
    name: z.string().optional(),
  },
  async ({ center, axis, base_radius, top_radius, height, name }) => {
    try {
      const r = await api("POST", "/api/geometry/cone", {
        center,
        axis,
        base_radius,
        top_radius,
        height,
        name: name ?? null,
      });
      const id = r.solid_id ?? (await newestPartId());
      return await okp(
        {
          object_uuid: r.object?.id ?? null, // operand for boolean / transform
          part_id: id,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "create_sphere",
  "ONE-CALL analytic sphere of `radius` at the origin. Returns part id + placement.",
  { radius: z.number().positive(), name: z.string().optional() },
  async ({ radius }) => {
    try {
      const r = await api("POST", "/api/geometry", {
        shape_type: "sphere",
        parameters: { radius },
      });
      const id = await newestPartId();
      return await okp(
        {
          object_uuid: r.object?.id ?? null, // operand for boolean / transform
          part_id: id,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "revolve",
  "Build a SOLID OF REVOLUTION from a closed meridian profile — the correct " +
    "primitive for any axisymmetric part (nozzles, pulleys, bottles, pressure " +
    "vessels, a whole rocket engine in one op). `profile` is a closed polygon of " +
    "[r, z] points (radius-from-axis, height-along-axis), revolved about the axis " +
    "(default +Z through origin, full 360°). One op, no booleans, watertight, " +
    "structured smooth mesh. Profile must be a simple loop with all r ≥ 0 and not " +
    "cross the axis (no r=0 pole). Hollow part = trace the wall cross-section.",
  {
    profile: z
      .array(z.tuple([z.number(), z.number()]))
      .min(3)
      .describe("closed [r,z] meridian profile (auto-closes last→first)"),
    axis_origin: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 0]),
    axis_direction: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 1]),
    angle_deg: z.number().default(360),
    segments: z.number().int().min(3).max(512).default(96),
    smooth: z
      .boolean()
      .optional()
      .describe(
        "fit a SMOOTH NURBS curve through `profile` (treated as the outer wall) so " +
          "the revolved wall is ONE surface, not a faceted polyline — needs bore_radius",
      ),
    bore_radius: z
      .number()
      .optional()
      .describe("hollow bore radius for a smooth-walled tube (use with smooth=true)"),
    wall_thickness: z
      .number()
      .optional()
      .describe(
        "CONTOURED nozzle/vessel (e.g. a Rao bell): treat `profile` as the INNER " +
          "flow contour and offset the outer wall by this thickness — both walls become " +
          "ONE smooth SurfaceOfRevolution (exact circles, smooth contour, no rings/seam).",
      ),
    name: z.string().optional(),
  },
  async ({
    profile,
    axis_origin,
    axis_direction,
    angle_deg,
    segments,
    smooth,
    bore_radius,
    wall_thickness,
    name,
  }) => {
    try {
      const r = await api("POST", "/api/geometry/revolve", {
        profile,
        axis_origin,
        axis_direction,
        angle_deg,
        segments,
        smooth: smooth ?? false,
        bore_radius: bore_radius ?? 0,
        wall_thickness: wall_thickness ?? 0,
        name: name ?? null,
      });
      const id = r.solid_id ?? (await newestPartId());
      return await okp(
        {
          object_uuid: r.object?.id ?? null, // operand for boolean / transform
          part_id: id,
          triangles: r?.stats?.triangle_count ?? null,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "nurbs_loft",
  "Build a watertight FREEFORM SOLID by skinning a single NURBS surface through " +
    "a stack of cross-section rings — the primitive for organic / sculpted shapes " +
    "that revolve and extrude can't make (bulged barrels, ogives, twisted/lobed " +
    "transitions, blended ducts). `sections` is an ordered list of cross-sections, " +
    "each an OPEN ring of [x,y,z] points of the SAME count (the op closes each " +
    "ring); sections are stacked along the loft. The lateral wall is one genuine " +
    "NURBS surface interpolated through the rings; at the default degree_v=3 it is " +
    "G2 (curvature-continuous) along the loft. The first and last sections must be " +
    "planar (they become the end caps). One op, no booleans, watertight.",
  {
    sections: z
      .array(z.array(z.tuple([z.number(), z.number(), z.number()])).min(3))
      .min(2)
      .describe("stack of cross-section rings (each an open ring, equal point count)"),
    degree_u: z.number().int().min(1).max(7).default(3).describe("degree around the section"),
    degree_v: z.number().int().min(1).max(7).default(3).describe("degree along the loft (3 = G2)"),
    name: z.string().optional(),
  },
  async ({ sections, degree_u, degree_v, name }) => {
    try {
      const r = await api("POST", "/api/geometry/nurbs_loft", {
        sections,
        degree_u,
        degree_v,
        name: name ?? null,
      });
      const id = r.solid_id ?? (await newestPartId());
      return await okp(
        {
          object_uuid: r.object?.id ?? null, // operand for boolean / transform
          part_id: id,
          triangles: r?.stats?.triangle_count ?? null,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "shell",
  "HOLLOW a solid to a constant wall thickness, opening the listed cap faces so " +
    "the interior cavity is exposed (a closed body becomes an open shell — a box " +
    "into a tray, a revolved/lofted body into a thin-walled vessel or nozzle). " +
    "`object` is the OBJECT UUID (the object_uuid returned by the create/loft/" +
    "revolve/boolean tools), NOT a kernel part id. `thickness` is the wall " +
    "thickness (the wall grows inward). `faces_to_remove` are the face ids to open " +
    "up — an empty list yields a fully closed hollow solid (a void), which is " +
    "rarely the intent. Get those face ids from `select_face` (e.g. " +
    "`{kind:'planar'}` returns the planar cap faces as candidates) or from a " +
    "render_part `mode:'ids'` legend. Identity-preserving: the body keeps its " +
    "UUID. Shelling can leave a self-intersecting or open wall, so ALWAYS " +
    "verify_part the result.",
  {
    object: z.string().uuid().describe("object_uuid of the solid to hollow"),
    thickness: z.number().describe("wall thickness (wall grows inward); must be non-zero"),
    faces_to_remove: z
      .array(z.number().int().nonnegative())
      .default([])
      .describe("face ids of the caps to open (from select_face / render_part ids); [] = closed void"),
  },
  async ({ object, thickness, faces_to_remove }) => {
    try {
      const r = await api("POST", "/api/geometry/shell", {
        object,
        thickness,
        faces_to_remove,
      });
      const part_id = r.solid_id ?? (await newestPartId());
      return await okp(
        {
          object_uuid: r.object?.id ?? object, // identity-preserving: same uuid
          part_id,
          triangles: r?.stats?.triangle_count ?? null,
          placement: part_id !== null ? await placement(part_id) : null,
        },
        part_id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "fillet_edges",
  "ROUND (fillet) one or more edges of a solid with a constant radius — the " +
    "blend that turns a sharp edge into a smooth cylindrical/spherical arc. Give " +
    "the kernel `part_id` (from list_parts) and the `radius` (> 0, in model " +
    "units). `edge_ids` selects which edges to blend (get them from select_edge / " +
    "render_part mode:'ids'); OMIT `edge_ids` to blend ALL edges of the solid at " +
    "once — including parts built through booleans (drilled holes, unions): a " +
    "hole's over-split rim is auto-healed into one edge and seams / over-radius " +
    "edges are skipped, so it ROUNDS EVERYTHING IT CAN rather than refusing the " +
    "whole op. Identity-preserving: the body keeps its part id/UUID. The result " +
    "is always perception-verified — check the returned cert (sound / watertight) " +
    "before trusting it.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    radius: z.number().positive().describe("fillet radius in model units (> 0)"),
    edge_ids: z
      .array(z.number().int().nonnegative())
      .optional()
      .describe(
        "edge ids to round (from select_edge / render_part ids); omit to blend ALL edges of the solid",
      ),
  },
  async ({ part_id, radius, edge_ids }) => {
    try {
      const object = await uuidForPart(part_id);
      const edges =
        edge_ids && edge_ids.length > 0 ? edge_ids : await allEdgeIds(part_id);
      const r = await api("POST", "/api/geometry/fillet", {
        object,
        edges,
        radius,
      });
      const id = r.solid_id ?? part_id;
      return await okp(
        {
          object_uuid: r.object?.id ?? object, // identity-preserving: same uuid
          part_id: id,
          filleted_edges: edges,
          all_edges: !(edge_ids && edge_ids.length > 0),
          triangles: r?.stats?.triangle_count ?? null,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "chamfer_edges",
  "BEVEL (chamfer) one or more edges of a solid with an equal-distance flat — the " +
    "blend that replaces a sharp edge with an angled face set back `distance` from " +
    "the edge on each adjacent face. Give the kernel `part_id` (from list_parts) " +
    "and the `distance` (> 0, in model units). `edge_ids` selects which edges to " +
    "bevel (get them from select_edge / render_part mode:'ids'); OMIT `edge_ids` " +
    "to chamfer ALL edges of the solid at once. Identity-preserving: the body " +
    "keeps its part id/UUID. Chamfering can self-intersect or fail to close at " +
    "tight corners, so the result is always perception-verified — check the " +
    "returned cert (sound / watertight) before trusting it.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    distance: z
      .number()
      .positive()
      .describe("chamfer setback distance in model units (> 0)"),
    edge_ids: z
      .array(z.number().int().nonnegative())
      .optional()
      .describe(
        "edge ids to bevel (from select_edge / render_part ids); omit to blend ALL edges of the solid",
      ),
  },
  async ({ part_id, distance, edge_ids }) => {
    try {
      const object = await uuidForPart(part_id);
      const edges =
        edge_ids && edge_ids.length > 0 ? edge_ids : await allEdgeIds(part_id);
      const r = await api("POST", "/api/geometry/chamfer", {
        object,
        edges,
        distance,
      });
      const id = r.solid_id ?? part_id;
      return await okp(
        {
          object_uuid: r.object?.id ?? object, // identity-preserving: same uuid
          part_id: id,
          chamfered_edges: edges,
          all_edges: !(edge_ids && edge_ids.length > 0),
          triangles: r?.stats?.triangle_count ?? null,
          placement: id !== null ? await placement(id) : null,
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "import_step",
  "IMPORT a STEP file (ISO 10303-21 / AP203 / AP214 / AP242) into the live " +
    "scene as one or more real B-Rep solids. Give either a `path` to a .step/.stp " +
    "file on disk OR inline `content` (the file text). The kernel reconstructs a " +
    "genuine shared B-Rep — planar/cylindrical/spherical/toroidal/conical faces, " +
    "rational & non-rational NURBS curves and surfaces, surfaces of revolution / " +
    "linear extrusion, solids with internal voids, and assembly (MAPPED_ITEM) " +
    "instances — then VALIDATES every solid (validate_solid_scoped) and folds the " +
    "verdict into ok. Returns the new part ids/uuids, the per-solid validity, and " +
    "the reconstruct-coverage report (resolved vs unsupported entity counts). " +
    "ok:false means a solid materialised but failed kernel validation — inspect " +
    "report.validation. Unsupported entities are listed honestly, never faked.",
  {
    path: z
      .string()
      .optional()
      .describe("filesystem path to a .step/.stp file (read locally by the MCP server)"),
    content: z
      .string()
      .optional()
      .describe("inline STEP file text (use instead of path)"),
    name: z.string().optional().describe("display-name prefix for imported parts"),
  },
  async ({ path, content, name }) => {
    try {
      let text = content;
      if (!text && path) {
        text = await readFile(path, "utf8");
      }
      if (!text) {
        return fail(new Error("provide either `path` or `content`"));
      }
      const r = await api("POST", "/api/geometry/import_step", {
        content: text,
        name: name ?? null,
      });
      const objects = Array.isArray(r.objects) ? r.objects : [];
      const id = await newestPartId();
      return await okp(
        {
          ok: r.success,
          imported: objects.map((o: any) => ({
            object_uuid: o.id,
            part_id: o.solid_id,
            name: o.name,
            brep_valid: o.perception?.brep_valid ?? null,
          })),
          coverage: {
            schema: r.report?.schema ?? null,
            roots_resolved: r.report?.roots_resolved ?? null,
            resolved: r.report?.counts?.resolved ?? null,
            unsupported: r.report?.counts?.unsupported ?? null,
            validation: r.report?.validation ?? null,
          },
          note:
            r.success === false
              ? "ok:false — a solid imported but failed kernel validation; see coverage.validation"
              : "imported; render_part / scene_view to SEE the result",
        },
        id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "section_view",
  "CUTAWAY: slice a part with a plane and return the cross-section as an image " +
    "(steel-filled profile + section area). The way to SEE a hollow interior. " +
    "Plane = point `p` + `normal`; an axial cut (normal ⟂ the part's axis through " +
    "its center) reveals wall thickness, bores and internal cavities.",
  {
    part_id: z.number().int(),
    p: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 0]),
    normal: z.tuple([z.number(), z.number(), z.number()]).default([1, 0, 0]),
  },
  async ({ part_id, p, normal }) => {
    try {
      const q = `nx=${normal[0]}&ny=${normal[1]}&nz=${normal[2]}&px=${p[0]}&py=${p[1]}&pz=${p[2]}`;
      const r = await api("GET", `/api/agent/parts/${part_id}/section?${q}`);
      return {
        content: [
          { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
          {
            type: "text" as const,
            text: `section area=${r.section_area?.toFixed?.(2)} extent_u=${r.extent_u?.toFixed?.(2)} extent_v=${r.extent_v?.toFixed?.(2)} units=${r.units}`,
          },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "dimension_part",
  "DIMENSION a part in ONE call. Returns a 2×2 multi-view image with every " +
    "analytic dimension drawn as a leader+label callout, AND the structured " +
    "table: each row has an id (the handle a future mould edits), kind " +
    "(extent / diameter / length / angle), value, the face ids it spans, and a " +
    "3D anchor. Values are read off analytic surfaces / exact curves, NEVER " +
    "measured from pixels — the id you SEE is the id you edit.",
  { part_id: z.number().int().describe("kernel part id from list_parts") },
  async ({ part_id }) => {
    try {
      const r = await api("GET", `/api/agent/parts/${part_id}/dimensions`);
      const rows = (r.dimensions ?? [])
        .map(
          (d: any) =>
            `${d.id}  ${d.label}  (${d.kind} ${d.value.toFixed(2)}${
              d.unit === "deg" ? "°" : ""
            })  faces=[${d.entities.join(",")}]  @[${d.anchor
              .map((c: number) => c.toFixed(1))
              .join(", ")}]`,
        )
        .join("\n");
      const overall = `overall L×W×H = ${r.dims.l.toFixed(2)} × ${r.dims.w.toFixed(
        2,
      )} × ${r.dims.h.toFixed(2)} ${r.units}`;
      return {
        content: [
          { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
          { type: "text" as const, text: `${overall}\n${rows}` },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

// `create_plate_with_holes` was RETIRED here: it composed the plate via the
// sketch+extrude path — the exact path documented to produce inside-out
// lateral meshes (⅓ volume, negative inertia), which is why create_box and
// create_cylinder were moved to analytic primitives. Its replacement is the
// honest pair: create_cylinder / create_box for the blank + `drill_pattern`
// (below) for the holes — analytic primitives and certified booleans only.


// ---- mutation: boolean (feature stacking) -----------------------------

server.tool(
  "boolean",
  "Combine two solids: union (weld together), difference (cut object_b out " +
    "of object_a), or intersection (keep only the overlap). Operands are " +
    "OBJECT UUIDs (the object_uuid returned by the create/extrude tools), " +
    "NOT kernel part ids. Both operands are consumed; a new solid is born. " +
    "This is feature-stacking — chained unions and bores. ALWAYS verify_part " +
    "the result: differences (bores/slots/counterbores) can leave open faces.",
  {
    op: z.enum(["union", "difference", "intersection"]),
    object_a: z.string().uuid().describe("object_uuid of the base solid"),
    object_b: z
      .string()
      .uuid()
      .describe("object_uuid of the tool solid (subtracted in difference)"),
  },
  async ({ op, object_a, object_b }) => {
    try {
      const r = await api("POST", "/api/geometry/boolean", {
        operation: op,
        object_a,
        object_b,
      });
      const part_id = await newestPartId();
      return await okp(
        {
          object_uuid: r.object?.id ?? null,
          part_id,
          consumed: r.consumed ?? [object_a, object_b],
          placement: part_id !== null ? await placement(part_id) : null,
        },
        part_id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

// ---- mutation: batch tools (one call, many features) -------------------

/** Standard plane name or custom {origin,u_axis,v_axis} → {o,u,v} basis. */
function resolvePlane(plane: any): { o: number[]; u: number[]; v: number[] } {
  const std: Record<string, { o: number[]; u: number[]; v: number[] }> = {
    xy: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 1, 0] },
    xz: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 0, 1] },
    yz: { o: [0, 0, 0], u: [0, 1, 0], v: [0, 0, 1] },
  };
  return typeof plane === "string"
    ? std[plane]
    : { o: plane.origin, u: plane.u_axis, v: plane.v_axis };
}

const cross3 = (a: number[], b: number[]) => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const unit3 = (a: number[]) => {
  const m = Math.hypot(a[0], a[1], a[2]);
  return [a[0] / m, a[1] / m, a[2] / m];
};

server.tool(
  "boolean_many",
  "BATCH boolean: apply ONE op (difference or union) of MANY tool solids " +
    "against a base, sequentially, in a single call — e.g. drill 9 bores " +
    "with one tool call instead of 9. Every step is ambient-certified; the " +
    "batch HALTS at the first step that leaves the base unsound and reports " +
    "which tool did it. The base keeps its uuid; consumed tools are gone.",
  {
    op: z.enum(["union", "difference"]),
    base: z.string().uuid().describe("object_uuid of the base solid"),
    tools: z
      .array(z.string().uuid())
      .min(1)
      .max(64)
      .describe("object_uuids applied in order; all consumed"),
  },
  async ({ op, base, tools }) => {
    try {
      let lastId: number | null = null;
      for (let i = 0; i < tools.length; i++) {
        // fast:true skips the endpoint's own full cert — the perceive() below
        // is the single certification gate per step (was 2× cert work/step).
        await api("POST", "/api/geometry/boolean", {
          operation: op,
          object_a: base,
          object_b: tools[i],
          fast: true,
        });
        lastId = await newestPartId();
        const p = await perceive(lastId);
        if (p && p.sound !== true) {
          return ok({
            object_uuid: base,
            part_id: lastId,
            completed: i + 1,
            of: tools.length,
            halted: `step ${i + 1} (${tools[i]}) left the base UNSOUND — ${compactVerdict(p)}`,
          });
        }
      }
      const p = await perceive(lastId);
      return ok({
        object_uuid: base,
        part_id: lastId,
        completed: tools.length,
        of: tools.length,
        placement: lastId !== null ? await placement(lastId) : null,
        perception: p ? compactVerdict(p) : null,
      });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "drill_pattern",
  "ONE-CALL hole pattern: drill `count` bores of radius `hole_r` on a ring " +
    "of radius `ring_r` (centered at cx,cy on `plane`) through a target " +
    "solid — creates the bore cylinders AND subtracts them, all in a single " +
    "call (9 holes = 1 call, not 18). Bores run along the plane normal " +
    "starting `z_offset` below the plane for `depth` — size them to " +
    "OVERSHOOT both faces of the target. REFUSES overlapping adjacent holes " +
    "up front (2·ring_r·sin(π/count) must exceed 2·hole_r): that regime " +
    "drives a known open boolean bug. Every drill is ambient-certified and " +
    "the batch halts on the first unsound step. Replaces the retired " +
    "create_plate_with_holes.",
  {
    object: z.string().uuid().describe("object_uuid of the solid to drill"),
    plane: PlaneSchema.default("xy"),
    cx: z.number().default(0).describe("pattern center (plane coords)"),
    cy: z.number().default(0),
    count: z.number().int().min(1).max(64),
    ring_r: z.number().positive().describe("ring radius the hole centers sit on"),
    hole_r: z.number().positive(),
    depth: z
      .number()
      .positive()
      .describe("bore length along the plane normal (overshoot the part!)"),
    z_offset: z
      .number()
      .default(-1)
      .describe("bore start along the normal (default −1 = 1mm under the plane)"),
    start_angle_deg: z.number().default(0),
  },
  async ({
    object,
    plane,
    cx,
    cy,
    count,
    ring_r,
    hole_r,
    depth,
    z_offset,
    start_angle_deg,
  }) => {
    try {
      // The same guard the kernel's exploration recipe uses: adjacent holes
      // whose chord spacing is below 2·hole_r intersect each other, and the
      // resulting cyl∩cyl saddle boolean is a known open kernel bug — refuse
      // loudly instead of hanging.
      if (count >= 2) {
        const spacing = 2 * ring_r * Math.sin(Math.PI / count);
        if (spacing <= 2 * hole_r) {
          return fail(
            new Error(
              `REFUSED: ${count} holes of r=${hole_r} on a ring of r=${ring_r} ` +
                `are spaced ${spacing.toFixed(3)} < 2·r=${(2 * hole_r).toFixed(3)} ` +
                `(adjacent holes intersect). Reduce count/hole_r or grow ring_r.`,
            ),
          );
        }
      }
      const p = resolvePlane(plane);
      const n = unit3(cross3(p.u, p.v));
      const bores: string[] = [];
      for (let k = 0; k < count; k++) {
        const th = ((start_angle_deg + (360 * k) / count) * Math.PI) / 180;
        const hx = cx + ring_r * Math.cos(th);
        const hy = cy + ring_r * Math.sin(th);
        const center = [0, 1, 2].map(
          (i) => p.o[i] + hx * p.u[i] + hy * p.v[i] + z_offset * n[i],
        );
        // fast:true — bore blanks are analytic primitives; the difference
        // step's perceive() certifies the merged result that actually matters.
        const r = await api("POST", "/api/geometry/cylinder", {
          center,
          axis: n,
          radius: hole_r,
          height: depth,
          name: `bore ${k + 1}/${count}`,
          fast: true,
        });
        const uuid = r.object?.id;
        if (!uuid) throw new Error(`bore ${k + 1}/${count}: no uuid returned`);
        bores.push(uuid);
      }
      let lastId: number | null = null;
      for (let k = 0; k < bores.length; k++) {
        // fast:true — perceive() below is the single per-hole cert gate.
        await api("POST", "/api/geometry/boolean", {
          operation: "difference",
          object_a: object,
          object_b: bores[k],
          fast: true,
        });
        lastId = await newestPartId();
        const pv = await perceive(lastId);
        if (pv && pv.sound !== true) {
          return ok({
            object_uuid: object,
            part_id: lastId,
            holes_completed: k + 1,
            of: count,
            halted: `hole ${k + 1} left the part UNSOUND — ${compactVerdict(pv)}`,
          });
        }
      }
      const pv = await perceive(lastId);
      return ok({
        object_uuid: object,
        part_id: lastId,
        holes: count,
        placement: lastId !== null ? await placement(lastId) : null,
        perception: pv ? compactVerdict(pv) : null,
      });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "transform",
  "Move and/or rotate a solid IN PLACE by its object_uuid. Identity is " +
    "preserved — same uuid, the viewport updates the existing object " +
    "rather than spawning a new one. Rotation (about an optional center, " +
    "default origin) applies first, then translation. Supply translation " +
    "and/or rotation (at least one). Angle is in DEGREES. After moving, " +
    "render_part to confirm the new position.",
  {
    object: z.string().uuid().describe("object_uuid of the solid to move"),
    translation: z
      .tuple([z.number(), z.number(), z.number()])
      .optional()
      .describe("[dx, dy, dz] world-space offset"),
    rotation: z
      .object({
        axis: z
          .tuple([z.number(), z.number(), z.number()])
          .describe("rotation axis (need not be unit length)"),
        angle_deg: z.number().describe("rotation angle in DEGREES"),
        center: z
          .tuple([z.number(), z.number(), z.number()])
          .optional()
          .describe("pivot point, default origin [0,0,0]"),
      })
      .optional(),
  },
  async ({ object, translation, rotation }) => {
    try {
      if (!translation && !rotation) {
        return fail(new Error("provide translation and/or rotation"));
      }
      const body: any = { object };
      if (translation) body.translation = translation;
      if (rotation) {
        body.rotation = {
          axis: rotation.axis,
          angle: (rotation.angle_deg * Math.PI) / 180,
          ...(rotation.center ? { center: rotation.center } : {}),
        };
      }
      const r = await api("POST", "/api/geometry/transform", body);
      return ok({
        object_uuid: r.object ?? object,
        moved: true,
        note: "render_part to confirm the new position",
      });
    } catch (e) {
      return fail(e);
    }
  },
);

// ─── Parametric sketching (csketch — constraint-solver backed) ────────

server.tool(
  "psketch_create",
  "Create a PARAMETRIC sketch (constraint-solver backed, XY plane). " +
    "Use psketch_* tools to add entities/constraints, solve, and extrude. " +
    "Unlike create_sketch (click-draft), geometry here can be DIMENSIONED " +
    "exactly: add entities loosely, constrain, solve — the solver places " +
    "everything to machine precision.",
  {},
  async () => {
    try {
      const s = await api("POST", "/api/csketch", {});
      return ok({ csketch_id: s.id });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "psketch_add",
  "Add an entity to a parametric sketch. kind=point {x,y,fixed?}, " +
    "line {start,end point uuids}, circle {cx,cy,radius}, arc {cx,cy," +
    "radius,start_angle,end_angle}, rectangle {x1,y1,x2,y2}, polyline " +
    "{points[[x,y]...],closed}. Returns the entity id.",
  {
    csketch_id: z.string().uuid(),
    kind: z.enum(["point", "line", "circle", "arc", "rectangle", "polyline"]),
    params: z.record(z.unknown()),
  },
  async ({ csketch_id, kind, params }) => {
    try {
      const r = await api("POST", `/api/csketch/${csketch_id}/${kind}`, params);
      return ok({ id: r.id });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "psketch_constrain",
  "Add a constraint. geometric: Horizontal/Vertical/Parallel/" +
    "Perpendicular/Coincident/Tangent/Concentric/Equal etc on entities. " +
    "dimensional: {Distance: 80.0} / {Radius: 6.0} / {Angle: 1.57} etc. " +
    "entities = [{Line: uuid}] or [{Point: uuid}, {Point: uuid}] ...",
  {
    csketch_id: z.string().uuid(),
    constraint_type: z.record(z.unknown()),
    entities: z.array(z.record(z.string())),
  },
  async ({ csketch_id, constraint_type, entities }) => {
    try {
      const r = await api("POST", `/api/csketch/${csketch_id}/constraint`, {
        id: crypto.randomUUID(),
        constraint_type,
        entities,
        priority: "High",
        status: "Satisfied",
        name: null,
      });
      return ok({ constraint_id: r.id });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "psketch_solve",
  "Run the Newton-Raphson solver. Converged = geometry now satisfies " +
    "every constraint exactly. Returns status + solved entity positions.",
  { csketch_id: z.string().uuid() },
  async ({ csketch_id }) => {
    try {
      const report = await api("POST", `/api/csketch/${csketch_id}/solve`, {});
      const summary = await api("GET", `/api/csketch/${csketch_id}`);
      return ok({ status: report.status, points: summary.points });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "psketch_extrude",
  "Extrude the parametric sketch's closed regions into a solid. " +
    "Hole-aware (circles inside an outline become bores). On topology " +
    "errors the response names every gap/dangling endpoint so you can " +
    "repair the sketch surgically. Records a replayable timeline event.",
  {
    csketch_id: z.string().uuid(),
    distance: z.number(),
    name: z.string().optional(),
  },
  async ({ csketch_id, distance, name }) => {
    try {
      const r = await api("POST", `/api/csketch/${csketch_id}/extrude`, {
        distance,
        name: name ?? null,
      });
      const part_id = await newestPartId();
      return await okp(
        {
          object_uuid: r.object?.id ?? null, // pass to `boolean` as an operand
          part_id, // kernel id for render/verify/inspect tools
          solid_id: r.solid_id,
          triangles: r.stats?.triangle_count,
          regions: r.stats?.regions,
        },
        part_id,
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "timeline_scrub",
  "Look at the scene AS OF a past event — non-destructive (live scene " +
    "untouched). Returns object count + mesh stats at that moment. " +
    "branch 'main' unless exploring a fork.",
  { branch: z.string().default("main"), sequence: z.number().int() },
  async ({ branch, sequence }) => {
    try {
      const r = await api("GET", `/api/timeline/scrub/${branch}/${sequence}`);
      return ok({
        at_sequence: r.at_sequence,
        events_applied: r.events_applied,
        events_total: r.events_total,
        objects: (r.objects ?? []).map((o: any) => ({
          id: o.id,
          name: o.name,
          triangles: (o.mesh?.indices?.length ?? 0) / 3,
        })),
      });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "clear_timeline",
  "Reset a timeline branch to ZERO events and wipe the live model to " +
    "match — the one-shot 'start over'. Default branch 'main'. DESTRUCTIVE " +
    "and irreversible: the event ledger is rewritten, not undoable. Unlike " +
    "undo/truncate (which refuse the protected main trunk and need a " +
    "specific event), this clears the whole branch. Use clear_parts instead " +
    "if you only want an empty scene but a preserved history.",
  {
    branch_id: z
      .string()
      .default("main")
      .describe("branch to clear; 'main' is the trunk"),
  },
  async ({ branch_id }) => {
    try {
      // The endpoint seeds its own replay position, so a fresh per-call
      // session id is sufficient; the truncate is branch-scoped, not
      // session-scoped.
      const r = await api("POST", "/api/timeline/clear", {
        session_id: randomUUID(),
        branch_id,
      });
      return ok({
        events_removed: r.events_removed,
        model_reconciled: r.model_reconciled,
        branch_id: r.branch_id ?? branch_id,
      });
    } catch (e) {
      return fail(e);
    }
  },
);

// ═══════════════════════════════════════════════════════════════════════
// BLACKBOARD — agent/human shared notebook of editable, event-logged lines.
// Backend-persisted (GET/POST/PATCH/DELETE /api/blackboard*); a line added
// here shows up live in the frontend Blackboard panel. Kept as a delimited
// block at the end so concurrent edits to the tool list above don't conflict.
// ═══════════════════════════════════════════════════════════════════════

/** Wire shape of one Blackboard line (mirrors the frontend store). */
interface BlackboardLine {
  id: string;
  text: string;
  author: "user" | "agent";
  createdAt: number;
  updatedAt: number;
}

/**
 * Build the `?scope=…` / `?part_id=…` query suffix for the scoped Blackboard
 * routes. A `part_id` (a part UUID from list_parts, or its integer kernel id)
 * targets THAT part's own notebook; `scope` lets the agent address an
 * assembly (`assembly:<uuid>`) or the document (`document`). Omitting both
 * targets the document-wide notebook.
 */
function blackboardScopeQuery(part_id?: string, scope?: string): string {
  const p = new URLSearchParams();
  if (scope) p.set("scope", scope);
  else if (part_id) p.set("part_id", part_id);
  const s = p.toString();
  return s ? `?${s}` : "";
}

const SCOPE_ARGS = {
  part_id: z
    .string()
    .optional()
    .describe(
      "target THIS part's own notebook — a part UUID (object_uuid) or its " +
        "integer kernel part_id from list_parts. Omit for the document-wide " +
        "notebook.",
    ),
  scope: z
    .string()
    .optional()
    .describe(
      "explicit scope token: 'document', 'part:<uuid>', or " +
        "'assembly:<uuid>' (cross-part calcs). Wins over part_id.",
    ),
};

server.tool(
  "blackboard_add_entry",
  "WRITE a line to a part's Blackboard — the per-part agent/human notebook " +
    "that the human SEES live in the app. Each part has its OWN notebook, so " +
    "pass `part_id` to record a finding, derivation, or result on THAT part " +
    "(markdown+math, e.g. '$E = mc^2$'). Omit part_id for the document-wide " +
    "notebook; use `scope` for assembly-level (cross-part) calcs. `author` " +
    "defaults to 'agent'. Returns the created line's id.",
  {
    text: z.string().describe("markdown + $math$ source for the line"),
    author: z
      .enum(["agent", "user"])
      .default("agent")
      .describe("line origin; defaults to 'agent'"),
    ...SCOPE_ARGS,
  },
  async ({ text, author, part_id, scope }) => {
    try {
      const line = (await api("POST", "/api/blackboard/entries", {
        text,
        author,
        ...(part_id ? { part_id } : {}),
        ...(scope ? { scope } : {}),
      })) as BlackboardLine;
      return ok({ id: line.id, author: line.author, text: line.text });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "blackboard_edit_entry",
  "EDIT an existing Blackboard line in place by its id (from blackboard_list). " +
    "Replaces the line's text and logs an edit event; the change appears live " +
    "in the app. Pass the same `part_id`/`scope` you listed it under (omit to " +
    "find the line by id across notebooks). Returns the updated line.",
  {
    id: z.string().describe("line id from blackboard_list"),
    text: z.string().describe("new markdown + $math$ source for the line"),
    ...SCOPE_ARGS,
  },
  async ({ id, text, part_id, scope }) => {
    try {
      const line = (await api(
        "PATCH",
        `/api/blackboard/entries/${encodeURIComponent(id)}${blackboardScopeQuery(part_id, scope)}`,
        { text },
      )) as BlackboardLine;
      return ok({ id: line.id, author: line.author, text: line.text });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "blackboard_list",
  "READ a part's Blackboard: returns that notebook's current lines (id, " +
    "author, text) in document order. Pass `part_id` to read THAT part's " +
    "notebook (each part has its own); omit for the document-wide notebook, " +
    "or use `scope` for an assembly. Use it to see what the human wrote or to " +
    "fetch a line id before editing.",
  { ...SCOPE_ARGS },
  async ({ part_id, scope }) => {
    try {
      const snap = (await api(
        "GET",
        `/api/blackboard${blackboardScopeQuery(part_id, scope)}`,
      )) as {
        lines?: BlackboardLine[];
      };
      const lines = (snap.lines ?? []).map((l) => ({
        id: l.id,
        author: l.author,
        text: l.text,
      }));
      return ok({ count: lines.length, lines });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "blackboard_clear",
  "CLEAR one Blackboard notebook — remove every line and reset its event log. " +
    "Scoped: `part_id` clears THAT part's notebook only (others untouched); " +
    "omit for the document-wide notebook, `scope` for an assembly. Destructive " +
    "'start over'; does not touch geometry.",
  { ...SCOPE_ARGS },
  async ({ part_id, scope }) => {
    try {
      return ok(
        await api(
          "POST",
          `/api/blackboard/clear${blackboardScopeQuery(part_id, scope)}`,
          {},
        ),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

// ════════════════════════════════════════════════════════════════════════
// LABELLER TOOLS (added block — do not interleave with other edits)
// Human-readable NAMEs pinned to a topological entity (vertex/edge/face) or a
// cross-section plane, so the agent and the user share one vocabulary for the
// geometry. Backed by POST/GET /api/agent/parts/{id}/labels and the
// .../labels/{name}/resolve endpoint. Resolution REFUSES on unknown/ambiguous
// rather than guessing — the Pillar-3 discipline.
// ════════════════════════════════════════════════════════════════════════

server.tool(
  "label_create",
  "PIN a human-readable name to a feature so you and the user share one word " +
    "for it (e.g. call the min-radius face 'throat'). Target it three ways: by " +
    "id (`entity_id` + `kind` vertex/edge/face); by DESCRIPTION (`kind` " +
    "face|edge + a `selector` shaped exactly like select_face/select_edge — the " +
    "kernel resolves it or REFUSES, never guesses); or as a named cross-section " +
    "(`kind`:'section' + `origin` + `normal`). Re-using a name REPLACES it (you " +
    "are told `replaced:true`). Returns the resolved entity_id / plane. See it " +
    "on the part with render_part(labels:true).",
  {
    part_id: z.number().int(),
    name: z.string().min(1).describe("the label, e.g. 'throat' (unique per part)"),
    kind: z.enum(["vertex", "edge", "face", "section"]),
    entity_id: z
      .number()
      .int()
      .optional()
      .describe("attach by id (omit when using selector or section)"),
    selector: z
      .string()
      .optional()
      .describe(
        "attach by description: a JSON object string shaped like the select_face " +
          "/select_edge body, e.g. '{\"kind\":\"cylindrical\",\"extremal\":\"smallest_area\"}'",
      ),
    origin: z
      .tuple([z.number(), z.number(), z.number()])
      .optional()
      .describe("section only: a point on the cutting plane"),
    normal: z
      .tuple([z.number(), z.number(), z.number()])
      .optional()
      .describe("section only: the plane normal"),
    description: z.string().optional(),
  },
  async ({ part_id, name, kind, entity_id, selector, origin, normal, description }) => {
    try {
      let selectorObj: unknown = null;
      if (selector !== undefined && selector.trim().length > 0) {
        try {
          selectorObj = JSON.parse(selector);
        } catch {
          return fail(`selector is not valid JSON: ${selector}`);
        }
      }
      // Raw fetch: 400/404/409 are meaningful REFUSALS (empty name / not-found
      // / ambiguous selector), surfaced as structured JSON, not transport errors.
      const res = await fetch(`${BASE}/api/agent/parts/${part_id}/labels`, {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Roshera-Agent": "Claude" },
        body: JSON.stringify({
          name,
          kind,
          entity_id: entity_id ?? null,
          selector: selectorObj,
          origin: origin ?? null,
          normal: normal ?? null,
          description: description ?? null,
        }),
      });
      return ok(await res.json());
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "label_list",
  "LIST every label on a part: name, kind (vertex/edge/face/section), world " +
    "anchor, deterministic display color (#rrggbb), the kernel-MEASURED key " +
    "dimension { value, unit, kind, display } (e.g. Ø2.00 mm — null when none), " +
    "the GD&T conformance verdict (in_spec/out_of_spec/not_verified — null when no " +
    "frame), whether the label is stale, and any description — in name order. " +
    "Your map of the shared vocabulary for this part.",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/labels`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "label_resolve",
  "RESOLVE a label name back to the live entity (face/edge/vertex id) or section " +
    "plane it pins. REFUSES with not_found (unknown name) or dangling (the entity " +
    "was deleted) — it never returns a wrong entity. Use it to turn a name the " +
    "user said ('fillet the throat edge') into the concrete id an op needs.",
  {
    part_id: z.number().int(),
    name: z.string().min(1),
  },
  async ({ part_id, name }) => {
    try {
      const res = await fetch(
        `${BASE}/api/agent/parts/${part_id}/labels/${encodeURIComponent(name)}/resolve`,
        { headers: { "X-Roshera-Agent": "Claude" } },
      );
      return ok(await res.json());
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "propose_labels",
  "AUTO-PROPOSE labels (D3): the kernel recognizes features on a part (throat = " +
    "the minimum-radius station along the symmetry axis — works on a revolved " +
    "bell nozzle, not just analytic cylinders, exit = the axis-extremal planar " +
    "cap at either end, chamber = max-radius barrel, fillet = constant-radius " +
    "blend) and SUGGESTS a name plus the ASSERTION that pins it — it does NOT " +
    "apply them. You confirm one by calling label_create with the suggested name " +
    "and the returned `selector` as the selector arg (you own the name, the " +
    "kernel owns the claim). Returns { proposals: [{ suggested_name, kind, " +
    "confidence, rationale, selector }] }.",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/propose-labels`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "label_delete",
  "REMOVE a label from a part by name (fixes a mislabel that can't otherwise be " +
    "undone). 200 with deleted:true when it existed; 404 when there was no such " +
    "name — the kernel reports honestly rather than pretending it deleted " +
    "something.",
  {
    part_id: z.number().int(),
    name: z.string().min(1).describe("the label to remove"),
  },
  async ({ part_id, name }) => {
    try {
      return ok(
        await api(
          "DELETE",
          `/api/agent/parts/${part_id}/labels/${encodeURIComponent(name)}`,
        ),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "label_rename",
  "RENAME a label, preserving its binding (the entity/selector it pins and its " +
    "description). 404 when the old name is unknown; 409 when the new name is " +
    "already taken by a DIFFERENT label (delete that one first — the kernel never " +
    "silently clobbers a binding).",
  {
    part_id: z.number().int(),
    name: z.string().min(1).describe("the existing label name"),
    new_name: z.string().min(1).describe("the new name (unique per part)"),
  },
  async ({ part_id, name, new_name }) => {
    try {
      return ok(
        await api(
          "PATCH",
          `/api/agent/parts/${part_id}/labels/${encodeURIComponent(name)}`,
          { new_name },
        ),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

// ════════════════════════ END LABELLER TOOLS ════════════════════════════

// ════════════════════════ ASSEMBLY (#19) TOOLS ══════════════════════════
//
// TRUE assemblies: an assembly is a set of positioned part INSTANCES, NOT a
// boolean of everything into one solid. An instance is a reference to a part
// (by id) plus a world transform — the SAME part id can be instanced many
// times at different poses, reusing the geometry instead of copying it. This
// is the scaling primitive for big assemblies (a 100-part scene = 100
// instances over far fewer distinct parts). Phase 1 is positioned instances;
// mates are Phase 2.

/** Build a row-major 4×4 transform from a translation (+ optional axis-angle
 *  rotation in degrees about a unit axis). Both optional → identity. */
function buildTransform(
  position?: [number, number, number],
  rotation_deg?: number,
  rotation_axis?: [number, number, number],
): number[][] {
  const I = [
    [1, 0, 0, 0],
    [0, 1, 0, 0],
    [0, 0, 1, 0],
    [0, 0, 0, 1],
  ];
  let m = I.map((r) => r.slice());
  if (rotation_deg && rotation_axis) {
    const [ax, ay, az] = rotation_axis;
    const len = Math.hypot(ax, ay, az) || 1;
    const [x, y, z] = [ax / len, ay / len, az / len];
    const a = (rotation_deg * Math.PI) / 180;
    const c = Math.cos(a);
    const s = Math.sin(a);
    const t = 1 - c;
    // Rodrigues rotation matrix.
    m = [
      [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0],
      [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0],
      [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0],
      [0, 0, 0, 1],
    ];
  }
  if (position) {
    m[0][3] = position[0];
    m[1][3] = position[1];
    m[2][3] = position[2];
  }
  return m;
}

server.tool(
  "assembly_create",
  "Create a TRUE assembly: a named scene of positioned part INSTANCES (not a " +
    "boolean merge). Returns the assembly id. Add instances with " +
    "`assembly_add_instance`, then SEE the whole thing with `assembly_view`. " +
    "Instances REFERENCE parts by id and reuse their geometry — the same part " +
    "can be placed many times.",
  { name: z.string().min(1).describe("display name, e.g. 'gearbox'") },
  async ({ name }) => {
    try {
      const r = await api("POST", "/api/assembly", { name });
      return ok({ assembly_id: r.id, name });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "assembly_add_instance",
  "Place an INSTANCE of an existing part into an assembly at a world pose. The " +
    "part is REFERENCED by id (and reused) — call this twice with the same " +
    "part_id at different positions to instance one part many times, no copy. " +
    "Pose: give `position` [x,y,z] (mm) and optionally `rotation_deg` about " +
    "`rotation_axis`, OR a raw row-major `transform` 4×4. Returns the new " +
    "instance id plus the assembly's perception (instance_count, " +
    "unique_part_count = distinct parts, per-instance soundness, combined bbox).",
  {
    assembly_id: z.string().describe("assembly id from assembly_create"),
    part_id: z.number().int().describe("kernel part id from list_parts"),
    position: z
      .array(z.number())
      .length(3)
      .optional()
      .describe("world translation [x,y,z] mm"),
    rotation_deg: z.number().optional().describe("rotation angle in degrees"),
    rotation_axis: z
      .array(z.number())
      .length(3)
      .optional()
      .describe("unit axis for rotation_deg, e.g. [0,0,1]"),
    transform: z
      .array(z.array(z.number()).length(4))
      .length(4)
      .optional()
      .describe("raw row-major 4×4 (overrides position/rotation)"),
    name: z.string().optional().describe("placement name, e.g. 'wheel-FL'"),
    color: z
      .array(z.number().int().min(0).max(255))
      .length(3)
      .optional()
      .describe("per-instance RGB"),
  },
  async ({
    assembly_id,
    part_id,
    position,
    rotation_deg,
    rotation_axis,
    transform,
    name,
    color,
  }) => {
    try {
      const t =
        transform ??
        buildTransform(
          position as [number, number, number] | undefined,
          rotation_deg,
          rotation_axis as [number, number, number] | undefined,
        );
      const r = await api(
        "POST",
        `/api/assembly/${assembly_id}/instance`,
        { part_id, transform: t, name, color },
      );
      return ok(r);
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "assembly_list_instances",
  "List an assembly's instances with full PERCEPTION: instance_count, " +
    "unique_part_count (distinct parts — the gap from instance_count is the " +
    "reuse), each instance's part_id / transform / resolved solid / soundness " +
    "(from the part's validity certificate), the combined world bbox, and " +
    "all_sound. This is how you confirm instancing is real (one part, many " +
    "instances) and that every placed part is a sound solid.",
  { assembly_id: z.string() },
  async ({ assembly_id }) => {
    try {
      return ok(await api("GET", `/api/assembly/${assembly_id}`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "assembly_transform_instance",
  "Re-pose ONE instance in an assembly without touching the others or the " +
    "referenced part. Give `position`/`rotation_deg`/`rotation_axis` or a raw " +
    "`transform`. Returns the updated assembly perception.",
  {
    assembly_id: z.string(),
    instance_id: z.string().describe("instance id from assembly_list_instances"),
    position: z.array(z.number()).length(3).optional(),
    rotation_deg: z.number().optional(),
    rotation_axis: z.array(z.number()).length(3).optional(),
    transform: z.array(z.array(z.number()).length(4)).length(4).optional(),
  },
  async ({
    assembly_id,
    instance_id,
    position,
    rotation_deg,
    rotation_axis,
    transform,
  }) => {
    try {
      const t =
        transform ??
        buildTransform(
          position as [number, number, number] | undefined,
          rotation_deg,
          rotation_axis as [number, number, number] | undefined,
        );
      return ok(
        await api(
          "PATCH",
          `/api/assembly/${assembly_id}/instance/${instance_id}`,
          { transform: t },
        ),
      );
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "assembly_view",
  "SEE A WHOLE ASSEMBLY: composite EVERY instance into one image, each part " +
    "rendered at its instance transform (reused geometry, not copied), from an " +
    "orbit camera (azimuth/elevation, world-Z up). This is the scene-eye for a " +
    "named assembly — the way to look at a positioned multi-part scene as a " +
    "whole. Change az/el to orbit. `mode:'diagnostic'` highlights open (red) / " +
    "non-manifold (magenta) edges.",
  {
    assembly_id: z.string(),
    az: z.number().default(35).describe("azimuth degrees around world Z"),
    el: z.number().default(20).describe("elevation degrees above horizon"),
    mode: z
      .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
      .default("shaded"),
    size: z.number().int().min(64).max(2048).default(720),
    quality: z.enum(["coarse", "medium", "fine"]).default("medium"),
  },
  async ({ assembly_id, az, el, mode, size, quality }) => {
    try {
      const r = await api(
        "GET",
        `/api/assembly/${assembly_id}/view?az=${az}&el=${el}&mode=${mode}&size=${size}&quality=${quality}`,
      );
      return {
        content: [
          { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
          {
            type: "text" as const,
            text:
              `assembly az=${az}° el=${el}° instances=${r.instance_count} ` +
              `distinct_parts=${r.unique_part_count} open=${r.open_edges} nm=${r.nonmanifold_edges}`,
          },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

// ════════════════════════ END ASSEMBLY TOOLS ════════════════════════════

// ─── main ──────────────────────────────────────────────────────────────

const transport = new StdioServerTransport();
await server.connect(transport);
console.error(`roshera-mcp connected (API: ${BASE})`);
