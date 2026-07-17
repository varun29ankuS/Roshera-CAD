/**
 * Shared plumbing for every Roshera MCP tool module: the HTTP client (with
 * bounded timeouts), the ambient-perception pipeline (embedded-verdict reuse,
 * compact one-line verdicts), result helpers, and small geometry utilities.
 *
 * Server URL via ROSHERA_URL (default http://localhost:8081).
 */

import { z } from "zod";

export const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";

// Backend credential. The MCP authorization spec directs stdio servers
// (which this is) AWAY from OAuth and toward reading a credential from
// the environment, so the API key is taken from ROSHERA_API_KEY and sent
// as `Authorization: ApiKey <key>` — the scheme the backend's
// auth_middleware parses (session-manager verify_api_key).
//
// When ROSHERA_API_KEY is unset, no Authorization header is sent. That
// still works against a backend running the local insecure bypass
// (ROSHERA_DEV_INSECURE=1), but a default (secure) backend will reject
// every request with 401 — set ROSHERA_API_KEY when driving any backend
// that enforces authentication. Computed once at module load; changing
// the key requires an MCP reconnect (`/mcp`), which restarts this
// process and re-reads the environment.
const API_KEY = process.env.ROSHERA_API_KEY;
export const AUTH_HEADERS: Record<string, string> = API_KEY
  ? { Authorization: `ApiKey ${API_KEY}` }
  : {};

// ─── HTTP helpers ──────────────────────────────────────────────────────

export class ApiError extends Error {
  constructor(message: string, public status: number, public body: string) {
    super(message);
  }
}

// Per-request timeout. A heavy kernel op (boolean over a complex part, fine
// tessellation, full re-cert) can legitimately take many seconds; the default
// is generous so we never abort a real computation, but it is bounded so a
// genuinely wedged backend surfaces as a clear 504 rather than hanging the
// agent forever. Override per process with ROSHERA_MCP_TIMEOUT_MS.
export const TIMEOUT_MS = (() => {
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
export const PERCEPTION_TIMEOUT_MS = (() => {
  const raw = process.env.ROSHERA_MCP_PERCEPTION_TIMEOUT_MS;
  const n = raw !== undefined ? Number(raw) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 4000;
})();

export async function api(
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
        // Credential (empty object when ROSHERA_API_KEY is unset).
        ...AUTH_HEADERS,
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

// ─── Document-unit cache (display-only; geometry stays mm-native) ──────

/**
 * Display-unit facts for the current document. Null until first use or a
 * document_units tool call. Refreshed whenever document_units GETs or PATCHes
 * the endpoint; lazily populated on the first compactVerdict call via a
 * best-effort GET (PERCEPTION_TIMEOUT_MS budget; failure stays null so the
 * fallback mm path is unchanged).
 *
 * Precision table (drafting convention, mirrors kernel units.rs):
 *   mm  → 1 dp   (today's compact verdict format)
 *   cm  → 3 dp
 *   m   → 4 dp   (kernel formatter parity)
 *   in  → 3 dp
 *   ft  → 4 dp   (kernel formatter parity)
 *
 * Volume converts as mm³ × perMm³ (perMm = the unit-per-millimetre factor).
 */
export interface DocumentUnitInfo {
  token: string;   // "mm" | "cm" | "m" | "in" | "ft"
  suffix: string;  // display suffix, same as token
  perMm: number;   // how many of this unit equals 1 mm
  dp: number;      // decimal places for volume display
}

const UNIT_TABLE: Record<string, Omit<DocumentUnitInfo, "token">> = {
  mm: { suffix: "mm", perMm: 1,            dp: 1 },
  cm: { suffix: "cm", perMm: 0.1,          dp: 3 },
  m:  { suffix: "m",  perMm: 0.001,        dp: 4 },
  in: { suffix: "in", perMm: 1 / 25.4,     dp: 3 },
  ft: { suffix: "ft", perMm: 1 / 304.8,    dp: 4 },
};

/** Cached document unit; null = unknown (will fetch lazily). */
let documentUnit: DocumentUnitInfo | null = null;
/** True while a lazy fetch is in flight — prevents parallel stampede. */
let documentUnitFetching = false;

/**
 * Called by the document_units tool to prime or update the cache after any
 * GET or PATCH. `token` is the unit string returned by the backend.
 */
export function setDocumentUnitCache(token: string): void {
  const entry = UNIT_TABLE[token];
  if (entry) documentUnit = { token, ...entry };
}

/**
 * Best-effort lazy fetch of the document unit. Fires at most once at a time.
 * On failure, leaves `documentUnit` null (compact verdict falls back to mm).
 */
async function fetchDocumentUnitOnce(): Promise<void> {
  if (documentUnit !== null || documentUnitFetching) return;
  documentUnitFetching = true;
  try {
    const r = await api("GET", "/api/document/units", undefined, PERCEPTION_TIMEOUT_MS);
    if (r && typeof r.unit === "string") setDocumentUnitCache(r.unit);
  } catch {
    // best-effort: fallback stays mm
  } finally {
    documentUnitFetching = false;
  }
}

/**
 * Format a raw mm³ volume value for display in the document unit.
 * Falls back to today's `vol=...mm³` when the unit is unknown.
 */
export function formatVolume(mm3: number): string {
  const u = documentUnit;
  if (!u || u.token === "mm") return `vol=${mm3.toFixed(1)}mm³`;
  const converted = mm3 * Math.pow(u.perMm, 3);
  return `vol=${converted.toFixed(u.dp)}${u.suffix}³`;
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

export function ok(data: unknown) {
  const content: any[] = [
    { type: "text" as const, text: JSON.stringify(data, null, 2) },
  ];
  return { content };
}

export function fail(e: unknown) {
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
export async function placement(partId: number) {
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

export async function newestPartId(): Promise<number | null> {
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
export async function uuidForPart(partId: number): Promise<string> {
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
export async function allEdgeIds(partId: number): Promise<number[]> {
  const res = await fetch(`${BASE}/api/agent/parts/${partId}/select-edge`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...AUTH_HEADERS },
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
export async function perceive(partId: number | null): Promise<any> {
  lastPerceiveUnavailableReason = null;
  if (partId === null) {
    lastPerceiveUnavailableReason = "op produced no part id to certify";
    return undefined;
  }
  if (process.env.ROSHERA_MCP_AUTOVERIFY === "0") {
    lastPerceiveUnavailableReason = "ambient perception disabled (ROSHERA_MCP_AUTOVERIFY=0)";
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
  } catch (err) {
    // #37: THE reason a caller must never see when the perception field goes
    // missing from a tool response — stash WHY so `perceptionField()` can
    // surface a typed "⚠ cert unavailable: <reason>" instead of a silent
    // omission/null. A timeout here is the common case: `PERCEPTION_TIMEOUT_MS`
    // is deliberately short (4s default) so a slow cert can never hang the op
    // that requested it, but that means sequential rapid-fire calls (e.g.
    // drill_pattern's per-hole certify loop) can occasionally miss the window.
    const msg = err instanceof Error ? err.message : String(err);
    lastPerceiveUnavailableReason = `perception fetch failed: ${msg}`;
    return undefined;
  }
}

/**
 * Sidecar set by the most recent `perceive()` call, naming WHY it returned
 * `undefined` (disabled / no part id / timeout / network error). `undefined`
 * itself is a legitimate, silent-by-convention JS value everywhere else in
 * this codebase, but the ambient-perception WIRE FIELD must never degrade to
 * a bare `null`/absent key with no explanation (#37) — every call site that
 * surfaces a perception verdict to the agent should route through
 * `perceptionField()` below rather than hand-rolling `p ? compactVerdict(p) : null`.
 */
let lastPerceiveUnavailableReason: string | null = null;

/**
 * Render a `perceive()` result as a string that is ALWAYS present and NEVER
 * a silent `null` (#37 — the ambient-perception omission bug: drill_pattern's
 * 3rd sequential call returned `"perception": null` and a sketch_extrude
 * response omitted the field entirely, both traced to a fallible `perceive()`
 * outcome being dropped on the floor instead of explained). Call this
 * immediately after `await perceive(...)` — the reason sidecar is overwritten
 * by the NEXT `perceive()` call.
 */
export function perceptionField(pv: any): string {
  if (pv) return compactVerdict(pv);
  return `⚠ cert unavailable: ${lastPerceiveUnavailableReason ?? "unknown reason"}`;
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
export function compactVerdict(p: any): string {
  // Kick off a lazy unit fetch (no await — best-effort, won't affect this call
  // but primes the cache for the NEXT verdict so it converges quickly).
  void fetchDocumentUnitOnce();

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
  if (typeof p?.volume === "number") facts.push(formatVolume(p.volume));
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
 * `ROSHERA_MCP_AUTOVERIFY=0` is the master off switch, but even then the
 * `perception` field is present (a typed "disabled" string, never a missing
 * key) — #37: the field is ALWAYS in the response, degradation is always
 * explained, never silent.
 */
export async function okp(data: Record<string, unknown>, partId: number | null) {
  const perception = await perceive(partId);
  const mode = process.env.ROSHERA_AMBIENT_PERCEPTION ?? "compact";
  if (perception === undefined) {
    // #37: `perceive()` always records WHY before returning undefined
    // (disabled / no part id / timeout / network error) — surface that
    // reason instead of dropping the `perception` key from the response.
    return ok({ ...data, perception: perceptionField(perception) });
  }
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

// ─── Geometry / plane helpers ──────────────────────────────────────────

export const PlaneSchema = z
  .union([
    z.enum(["xy", "xz", "yz"]),
    z.object({
      origin: z.tuple([z.number(), z.number(), z.number()]),
      u_axis: z.tuple([z.number(), z.number(), z.number()]),
      v_axis: z.tuple([z.number(), z.number(), z.number()]),
    }),
  ])
  .describe("'xy' | 'xz' | 'yz' or {origin, u_axis, v_axis} (e.g. from plane_from_face)");

/** Standard plane name or custom {origin,u_axis,v_axis} → {o,u,v} basis. */
export function resolvePlane(plane: any): { o: number[]; u: number[]; v: number[] } {
  const std: Record<string, { o: number[]; u: number[]; v: number[] }> = {
    xy: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 1, 0] },
    xz: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 0, 1] },
    yz: { o: [0, 0, 0], u: [0, 1, 0], v: [0, 0, 1] },
  };
  return typeof plane === "string"
    ? std[plane]
    : { o: plane.origin, u: plane.u_axis, v: plane.v_axis };
}

export const cross3 = (a: number[], b: number[]) => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];

export const unit3 = (a: number[]) => {
  const m = Math.hypot(a[0], a[1], a[2]);
  return [a[0] / m, a[1] / m, a[2] / m];
};

// ─── File-save helpers (export / drawing fetch) ─────────────────────────

/** Save raw bytes fetched from a backend path to an absolute file on disk. */
export async function saveBinary(urlPath: string, savePath: string): Promise<number> {
  const res = await fetch(`${BASE}${urlPath}`, { headers: { ...AUTH_HEADERS } });
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
export async function defaultSaveDir(): Promise<string> {
  const { homedir } = await import("node:os");
  const { join } = await import("node:path");
  return join(homedir(), "Desktop");
}
