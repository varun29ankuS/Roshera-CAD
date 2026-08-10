/**
 * Shared plumbing for every Roshera MCP tool module: the HTTP client (with
 * bounded timeouts), the ambient-perception pipeline (embedded-verdict reuse,
 * compact one-line verdicts), result helpers, and small geometry utilities.
 *
 * Server URL via ROSHERA_URL (default http://localhost:8081).
 */

import { z } from "zod";
// Import cycle note: gates.ts imports `api` from this module; both imports
// are only dereferenced at call time (never during module evaluation) and
// both are hoisted function declarations, so the ESM cycle is benign.
import { currentOpenIntent } from "./gates.js";

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

/**
 * Intent provenance headers for one backend call. When an intent checkpoint
 * is open (gates.ts — the same state the intent gate enforces), every call
 * carries it so the backend's agent_intent_layer can scope it onto the
 * request task and the TimelineRecorder can stamp an IntentFacet onto the
 * kernel ops this request records. The name is free text (may contain
 * non-ASCII); HTTP header values must be ASCII, so it is URL-encoded here
 * and percent-decoded server-side. No open intent → no headers at all: an
 * absent intent stays absent on the wire, never defaulted.
 */
function intentHeaders(): Record<string, string> {
  const intent = currentOpenIntent();
  if (intent === null) return {};
  return {
    "X-Roshera-Intent": encodeURIComponent(intent.name),
    "X-Roshera-Intent-Turn": String(intent.turn),
  };
}

// ─── Session→document binding (2026-08-10) ──────────────────────────────
//
// Binds the goose session to its BIRTH document — the one active at process
// start — so a human opening another tab and switching the active document
// cannot silently retarget an in-flight agent mid-task. Read once at startup
// (index.ts fires `bindSessionDocument()` right after connect, alongside
// `consumeRegistry()`); every subsequent `api()` call carries it.
let boundDocument: string | null = null;

/**
 * RAW fetch (never `api()` — `api()` itself calls `documentHeaders()` below,
 * so routing this through `api()` would be a self-referential bootstrap) of
 * the document list; binds to whichever one is `active` at this moment. Best-
 * effort: any failure (network, non-OK status, no active document) leaves
 * `boundDocument` null, which is LEGACY behaviour — every call goes out
 * unbound, exactly as it did before this existed.
 */
export async function bindSessionDocument(): Promise<void> {
  try {
    const res = await fetch(`${BASE}/api/documents`, {
      headers: { ...AUTH_HEADERS },
      signal: AbortSignal.timeout(PERCEPTION_TIMEOUT_MS),
    });
    if (!res.ok) return;
    const docs = await res.json();
    if (!Array.isArray(docs)) return;
    boundDocument = docs.find((d: any) => d?.active)?.id ?? null;
  } catch {
    // swallow — unbound is legacy behaviour, never a hard failure.
  }
}

/** The bound-document header for one backend call, or `{}` when unbound. */
function documentHeaders(): Record<string, string> {
  return boundDocument === null ? {} : { "X-Roshera-Document": boundDocument };
}

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
        // Intent provenance: the open checkpoint phrase, when one exists.
        ...intentHeaders(),
        // Session→document binding: the birth-document id, when bound.
        ...documentHeaders(),
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
  // IMAGE-FRESHNESS INVALIDATION: any mutating call invalidates the whole-scene
  // cache (scene_view); one that reports a specific part id also invalidates
  // that part's own cache (render_part/section_view). See the "Image freshness"
  // section above core.ts for why this hook lives here (the same mutating-call
  // branch the embedded-perception stash already uses).
  if (method !== "GET") {
    recordGlobalMutation();
    const mutatedId =
      parsed && typeof parsed === "object"
        ? ((parsed as any).solid_id ?? (parsed as any).id)
        : undefined;
    if (typeof mutatedId === "number") recordPartMutation(mutatedId);
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
    // DOCUMENT-level durability disclosure, present ONLY when the backend's
    // response carried one (a QUARANTINED document — see `/perception`'s
    // `durability` field). Never fabricated; absent means nothing withheld.
    // `r.perception?.durability` is what makes this reachable on the
    // embedded-reuse path too: a mutating op's OWN response (`certified_response`
    // in main.rs) now embeds `durability` at `body.perception.durability` on a
    // quarantined document, so `r` being the raw mutating body (not just a
    // `/perception`-shaped response) already carries it here.
    durability: r.durability ?? r.perception?.durability ?? undefined,
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

/**
 * Parsed shape of the backend's structured error catalog wire body
 * (`api-server/src/error_catalog.rs` — `ApiError`'s `Serialize` impl):
 * `{ success: false, error_code, error, retryable, hint?, details? }`.
 * `error_code` is the field this parser treats as load-bearing: the
 * catalog's own module doc says the prose `error` field is "free to
 * evolve" but changing/removing a variant is a versioned break, so
 * `error_code` is the stable thing to branch on, not substrings of `error`.
 */
interface CatalogError {
  error_code: string;
  retryable: boolean;
  details?: unknown;
  hint?: string;
}

/**
 * Parse `ApiError.body` as the backend's typed error catalog wire shape.
 * Returns `null` for anything that isn't catalog-shaped: a non-JSON body
 * (a plain-text 500, a stub test backend that answers with raw text), a
 * JSON body with no `error_code`, or a client-constructed `ApiError` that
 * never reached the backend at all (the timeout/network branches in
 * `api()` above always pass `body: ""`). Never throws — a parse failure
 * degrades to "no structured error" rather than breaking error handling
 * itself, which would be the one place a crash is least affordable.
 */
function parseCatalogError(body: string): CatalogError | null {
  if (!body) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const o = parsed as Record<string, unknown>;
  if (typeof o.error_code !== "string") return null;
  return {
    error_code: o.error_code,
    retryable: o.retryable === true,
    details: "details" in o ? o.details : undefined,
    hint: typeof o.hint === "string" ? o.hint : undefined,
  };
}

/**
 * Compute a legible, structured-data-driven retry hint for the one catalog
 * code whose backend constructor deliberately carries NO `hint`:
 * `ApiError::blend_failed` (error_catalog.rs) is the only named constructor
 * that never calls `.with_hint(...)` — because the useful remediation is
 * arithmetic over `details.failure`, not prose the server could usefully
 * phrase once for every `BlendFailure` variant. So this MCP client computes
 * it instead of guessing from message text.
 *
 * Only `RadiusExceedsCurvature` (geometry-engine's `BlendFailure`,
 * `operations/diagnostics.rs`) carries an `r_max` an agent can act on
 * directly with one arithmetic step. `type` is documented there as part of
 * the wire contract ("changing it is a breaking change to the agent
 * surface"), so branching on it is as stable as branching on `error_code`
 * itself. Every other variant (`SetbackTooLong`, `DihedralInflection`,
 * `VertexBlendUnsupported`, `TopologyViolation`, …) returns `null` here and
 * falls through to the raw message / `errorHint` path below rather than
 * fabricate a radius that was never computed for it.
 */
function blendFailureHint(details: unknown): string | null {
  if (!details || typeof details !== "object") return null;
  const failure = (details as Record<string, unknown>).failure;
  if (!failure || typeof failure !== "object") return null;
  const f = failure as Record<string, unknown>;
  if (f.type !== "RadiusExceedsCurvature") return null;
  const rMax = f.r_max;
  if (typeof rMax !== "number") return null;
  const rReq = typeof f.r_requested === "number" ? f.r_requested : null;
  const edge = typeof f.edge === "number" ? ` at edge ${f.edge}` : "";
  const station =
    typeof f.station === "number" ? ` (station ${f.station.toFixed(3)})` : "";
  const requested = rReq !== null ? `requested radius ${rReq}mm exceeds` : "the requested radius exceeds";
  return (
    `${requested} the local curvature limit${edge}${station} — retry with ` +
    `radius ≤ ${rMax}mm (r_max), or pass edge_ids to blend only the ` +
    `edges that fit at the larger radius.`
  );
}

export function fail(e: unknown) {
  const msg = e instanceof Error ? e.message : String(e);
  const catalog = e instanceof ApiError ? parseCatalogError(e.body) : null;
  // Hint precedence: a computed structured hint (exact, derived from typed
  // `details` — today only blend_failed/RadiusExceedsCurvature) beats the
  // backend's own `hint` (still prose, but STABLE prose the server authored
  // specifically for this code) beats `errorHint`'s substring match below
  // (the only tier still coupled to `error` text the catalog's own doc says
  // is free to evolve).
  const structuredHint =
    catalog?.error_code === "blend_failed"
      ? blendFailureHint(catalog.details)
      : null;
  const hint = structuredHint ?? catalog?.hint ?? errorHint(msg);
  const result: {
    content: { type: "text"; text: string }[];
    isError: true;
    structuredContent?: Record<string, unknown>;
  } = {
    content: [
      {
        type: "text" as const,
        text: hint ? `ERROR: ${msg}\nHINT: ${hint}` : `ERROR: ${msg}`,
      },
    ],
    isError: true as const,
  };
  if (catalog) {
    // MCP's own structured-output carrier (`CallToolResult.structuredContent`
    // in the SDK's types.js). It survives with no `outputSchema` declared on
    // these tools — `McpServer.validateToolOutput` returns immediately when
    // `tool.outputSchema` is absent, and skips validation entirely on
    // `isError` results regardless — and is returned to the wire verbatim
    // (the CallToolRequestSchema handler does `return result;` with no
    // stripping), so an agent can read `error_code` / `retryable` / `details`
    // as typed fields instead of re-parsing `content[0].text` prose the way
    // every caller of this function had to before.
    result.structuredContent = {
      // The refusal-card wire shape (roshera-app/src/lib/blackboard-cards.ts
      // `refusalCardSchema`): `reason` is the backend message VERBATIM, so
      // this object validates as a `roshera:refusal` payload — the agent
      // (per .goosehints "payload EXACTLY as returned") or the app's
      // cardFenceForPayload can fence it unchanged, carrying error_code /
      // retryable / hint all the way to the rendered card.
      reason: msg,
      error_code: catalog.error_code,
      retryable: catalog.retryable,
      ...(hint !== null ? { hint } : {}),
      ...(catalog.details !== undefined ? { details: catalog.details } : {}),
    };
  }
  return result;
}

/**
 * Translate a common kernel refusal into ONE actionable next step. The kernel
 * refuses rather than ship bad geometry (the moat); this turns its terse,
 * correct error into guidance the agent can act on. Returns null when the raw
 * message is already clear.
 *
 * FALLBACK TIER ONLY (see `fail()` above): reached when the response body
 * carried no `error_code` at all (a client-constructed timeout/network
 * `ApiError`, whose `body` is always `""`; or a raw non-catalog 500) or a
 * catalog `error_code` this module does not yet special-case. Every branch
 * below is matched against `error` prose the catalog's own doc (`error_catalog.rs`
 * lines 10-11) declares free to evolve — `test/structured_errors.test.mjs`
 * pins the still-live producer phrases so a backend rename breaks loudly here
 * instead of silently degrading hint quality.
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
  if (m.includes("disjoint"))
    return "the two solids do not touch, so the boolean would change nothing — check both placements with get_part (world center + dimensions, mm) and move one with transform before retrying.";
  if (m.includes("stale") || m.includes("has been mutated"))
    return "the part changed since its last full verification — run verify_part({part_id}) to re-certify, then retry this call.";
  if (
    m.includes("unsound") ||
    m.includes("non-manifold") ||
    m.includes("not certified")
  )
    return "the kernel produced an unsound result and refused it (the moat held) — inspect with verify_part / ground_truth; do NOT assume the geometry is valid.";
  if (m.includes("401") || m.includes("unauthorized"))
    return "the backend refused the credential — set ROSHERA_API_KEY to a valid key and reconnect the MCP (the key is read once at process start).";
  if (m.includes("no live solid") || m.includes("not found"))
    return "that id no longer names a live solid: every mutating op (boolean, shell, blend) MINTS a NEW id and CONSUMES its inputs, and its result carries the current object_uuid + part_id. Chain off your most recent op result, or call list_parts for the current integer ids.";
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
    `no live solid has part_id ${partId} — mutating ops (boolean, shell, blend) ` +
      `mint a NEW id and consume their inputs, so an id from before such an op ` +
      `is dead. Use the ids from the op's own result, or list_parts for the ` +
      `current set.`,
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
      // DOCUMENT-level durability disclosure straight off this GET /perception
      // response — present ONLY on a QUARANTINED document. The FAST PATH above
      // (reused from a mutating op's own embedded response, via
      // `perceptionFromBody`) carries the same field too: `certified_response`
      // (api-server/src/main.rs) now embeds `durability` under
      // `body.perception.durability` on a quarantined document, which
      // `perceptionFromBody`'s `r.perception?.durability` picks up.
      durability: p?.durability ?? undefined,
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
  // DOCUMENT-level context, prefixed loudly and BESIDE the part verdict below
  // — never softens or replaces it. Present only when the fetch this verdict
  // was built from carried a `durability` field (a QUARANTINED document: a
  // slice of this document's recorded history could not be replayed and was
  // refused, not silently served). See `p.durability` for the full state
  // (first_break_kind/reason/events_served/events_total).
  const durabilityNote = p?.durability
    ? `⚠ DOCUMENT QUARANTINED (${p.durability.reason ?? "history incomplete — see p.durability"}) | `
    : "";
  if (p?.sound === true && failed.length === 0) {
    const verified = DIMS.filter(([k]) => p?.[k] === true).map(([, n]) => n);
    const suffix = unverified.length
      ? ` (unverified: ${unverified.join(",")} — verify_part to certify)`
      : "";
    return `${durabilityNote}SOUND ✓ ${verified.join("·")}${suffix}${tail}`;
  }
  const why = failed.length ? failed.join(", ") : "cheap verdict false";
  return `${durabilityNote}UNSOUND ✗ failed: ${why}${tail} — run verify_part for the full certificate + diagnostic render`;
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

// ─── Image freshness / turn-count expiry (2026-08-01) ──────────────────
//
// WHY THIS EXISTS: the live provider (goose's `claude-code`) spawns one
// persistent `claude` CLI per session and sets `manages_own_context=true` —
// the CLI keeps its OWN internal conversation state across turns; goose never
// resends it (confirmed in providers/claude_code.rs: `last_user_content_blocks`
// forwards only the latest user turn, and even there it strips images out of
// tool-response content, extracting Text blocks only). The CLI itself talks to
// this MCP server directly (`--mcp-config`/`--strict-mcp-config`), so every
// tool result — including every image — lands straight in the CLI's own
// history. The CLI has no exposed retention/compaction flag (`claude --help`
// audited clean). Once an image is sent, THIS PROCESS CANNOT UN-SEND IT — the
// only lever available anywhere in the stack is deciding whether the NEXT call
// attaches pixels at all. That is what this section does.
//
// POLICY: a view's pixels count as "the live image" for IMAGE_TTL_TURNS turns
// (a turn = one tool-call dispatch, ticked by ToolTable for every call on
// every path — direct, invoke(), cad_program()) UNLESS a mutation that could
// change those pixels happens first, which invalidates immediately regardless
// of the TTL. Either trigger (mutation or TTL) causes the NEXT call for that
// view to mint a REAL fresh image — expiry only ever produces MORE pixels,
// never fewer, so a genuine look is never blocked. Within the window, with no
// mutation, a repeat call is a no-op for pixels (an identical image is already
// sitting in the CLI's context) but the TEXT VERDICT is always recomputed
// fresh from the backend and always included — only pixels are ever withheld,
// never the verdict — and the response says so explicitly (never silence),
// naming the turn the standing image was taken and when it refreshes.
//
// ONE PLACE the number lives: ROSHERA_MCP_IMAGE_TTL_TURNS env var, default 3.
export const IMAGE_TTL_TURNS = (() => {
  const raw = process.env.ROSHERA_MCP_IMAGE_TTL_TURNS;
  const n = raw !== undefined ? Number(raw) : NaN;
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : 3;
})();

let turnCounter = 0;

/** Advance the session's turn counter by one tool-call dispatch. Called once
 *  per handler invocation by ToolTable (registry.ts) — the single choke point
 *  every call path (direct mount, invoke(), cad_program()) runs through. */
export function nextTurn(): number {
  turnCounter += 1;
  return turnCounter;
}

/** Turn a given kernel part id was last touched by a mutating call. */
const mutatedAtByPart = new Map<number, number>();
/** Turn ANY mutating call last touched ANY part — scene_view's cache key,
 *  since a composite of the whole scene has no single part to watch. */
let lastGlobalMutationTurn = 0;

/** Record that `partId` was just mutated — invalidates its cached image. */
export function recordPartMutation(partId: number): void {
  mutatedAtByPart.set(partId, turnCounter);
}
/** Record that some mutating call happened — invalidates scene_view's cache. */
export function recordGlobalMutation(): void {
  lastGlobalMutationTurn = turnCounter;
}
/** The invalidation watermark for a specific part's cached image. */
export function lastPartMutationTurn(partId: number): number {
  return mutatedAtByPart.get(partId) ?? 0;
}
/** The invalidation watermark for the whole-scene cached image. */
export function lastGlobalMutationTurnValue(): number {
  return lastGlobalMutationTurn;
}

interface ImageCacheEntry {
  /** Turn real pixels were last sent for this key. */
  turn: number;
  /** The mutation watermark in force at send time. */
  watermark: number;
  /** Short description of what was shown, echoed in the expiry note. */
  note: string;
}
const imageCache = new Map<string, ImageCacheEntry>();

/**
 * Decide whether a call to an image-returning tool should attach real pixels.
 * `key` names the logical view (tool+part+params); `mutationWatermark` is the
 * CURRENT invalidation watermark for that view's scope (a specific part via
 * `lastPartMutationTurn`, or the whole scene via `lastGlobalMutationTurnValue`).
 */
export function imageFreshness(
  key: string,
  mutationWatermark: number,
): { send: true } | { send: false; note: string } {
  const prev = imageCache.get(key);
  if (!prev) return { send: true };
  const mutatedSince = mutationWatermark > prev.watermark;
  const ttlElapsed = turnCounter - prev.turn >= IMAGE_TTL_TURNS;
  if (mutatedSince || ttlElapsed) return { send: true };
  return {
    send: false,
    note:
      `a view was taken at turn ${prev.turn} (${prev.note}) and nothing has ` +
      `changed since — pixels withheld, not resent (still identical, already ` +
      `in context); refreshes automatically on the next mutation or by turn ` +
      `${prev.turn + IMAGE_TTL_TURNS}. Verdict below is freshly computed, not cached.`,
  };
}

/** Record that real pixels were just sent for `key`. */
export function recordImageSent(
  key: string,
  mutationWatermark: number,
  note: string,
): void {
  imageCache.set(key, { turn: turnCounter, watermark: mutationWatermark, note });
}

// ─── Geometry / plane helpers ──────────────────────────────────────────

export const PlaneSchema = z
  .union([
    z.enum(["xy", "xz", "yz"]),
    z.object({
      origin: z.array(z.number()).length(3).describe("plane origin [x,y,z] mm"),
      u_axis: z.array(z.number()).length(3).describe("plane in-plane x axis [x,y,z]"),
      v_axis: z.array(z.number()).length(3).describe("plane in-plane y axis [x,y,z]"),
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
