/**
 * The real MCP session: a stdio client against a `roshera-mcp` process
 * PINNED to one document via ROSHERA_DOCUMENT.
 *
 * The pin is what makes parallel episodes possible. Without it every process
 * discovers the globally-`active` document and they all land on the same one
 * (roshera-mcp/src/core.ts::bindSessionDocument, core.ts:116-138).
 *
 * ─── THE WIRE CONTRACT THIS MODULE SPEAKS ────────────────────────────────
 *
 * Every shape below was read from source; nothing here is inferred from a
 * tool's prose description. A tool result is an MCP `CallToolResult`, and the
 * Roshera surface produces exactly four kinds of them:
 *
 *   1. SUCCESS — `core.ts:380-385 ok(data)`:
 *        { content: [{ type:"text", text: JSON.stringify(data, null, 2) }] }
 *      No `isError`, no `structuredContent`.
 *   2. FAILURE — `core.ts:471-523 fail(e)`:
 *        { content: [{ type:"text", text: "ERROR: <msg>\nHINT: <hint>" }],
 *          isError: true, structuredContent?: { reason, error_code,
 *          retryable, hint?, details? } }
 *      The text is PROSE, so `JSON.parse` throws on it. `structuredContent`
 *      is present only when the backend body was catalog-shaped
 *      (`error_code` present — core.ts:413-430).
 *   3. GATE REFUSAL — `gates.ts:121-131 gateRefusal(payload)`:
 *        { content: [{ type:"text", text: JSON.stringify({refused:true, …}) }],
 *          isError: true }
 *   4. TIMELINE REFUSAL — `tools/timeline.ts:26-35 refusalOrFail(e)`:
 *        `ok({ refused: <PARSED BACKEND BODY> })` — `refused` is an OBJECT,
 *        not `true`, and the result carries NO `isError`.
 *
 * `readToolResult` below normalises all four into ONE envelope and is
 * deliberately a PURE function so it can be tested against results copied
 * verbatim from those sources without an SDK, a child process or a backend.
 * `verifyClaims` and `fetchRecipe` are exported and `call`-parameterised for
 * the same reason: the terminal-scoring logic is testable on its own.
 *
 * Every call is dispatched through the `invoke` meta-tool — see the comment on
 * `call` in `spawnMcpSession` for why that is a correctness requirement and
 * not a stylistic choice (the default surface mounts non-core tools DISABLED,
 * and `verify_claim` / `recipe_get` are both non-core).
 *
 * The SDK imports inside `spawnMcpSession` are DYNAMIC, not top-level.
 * `@modelcontextprotocol/sdk` is not installed in every environment this
 * module gets imported into (in particular: unit tests that inject a fake
 * session and never touch a real MCP process). Loading it eagerly at module
 * scope would make the whole episode module unimportable wherever the SDK is
 * absent, which is exactly where the fake-session tests need it to import
 * cleanly.
 */
import { fileURLToPath } from "node:url";

/**
 * Detect a typed refusal in ANY tool result, whatever path produced it.
 *
 * MIRROR — this is `roshera-mcp/src/gates.ts:142-163 typedRefusalOf()`,
 * copied deliberately rather than reimplemented. It is not exported from
 * gates.ts (and gates.ts is TypeScript compiled into a bundle this package
 * does not depend on), so sharing the symbol is not available; a NARROWER
 * copy is what produced CRITICAL 1, so the copy is exact and
 * `test/mcp_session.test.mjs` pins the load-bearing line against drift the
 * same way the repo's ontology-drift gate pins two independently-maintained
 * surfaces to each other.
 *
 * The three cases it covers, and why each exists:
 *  - `refused === true` — kb_lookup and every gate in gates.ts,
 *  - `refused` is an OBJECT — the timeline tools' `ok({refused: <body>})`,
 *  - an `isError` result whose text carries the kernel's REFUSED marker —
 *    drill_pattern's spacing guard and backend typed refusals.
 *
 * Returns the parsed gate name when one exists, `{gate: null}` for a refusal
 * that names no gate, and `null` for a non-refusal.
 */
function typedRefusalOf(result) {
  const content = Array.isArray(result?.content) ? result.content : [];
  const first = content.find((c) => c?.type === "text" && typeof c.text === "string");
  if (!first) return null;
  const text = first.text;
  try {
    const data = JSON.parse(text);
    if (data && typeof data === "object") {
      const r = data.refused;
      if (r === true || (r !== null && typeof r === "object")) {
        const gate = data.gate;
        return { gate: typeof gate === "string" ? gate : null };
      }
    }
  } catch {
    // not JSON — fall through to the marker check
  }
  if (result?.isError === true && /\bREFUSED\b/.test(text)) return { gate: null };
  return null;
}

/**
 * Did the SHARED rate class refuse this call?
 *
 * `ApiError.status` (core.ts:37-41) lives inside the MCP child process and
 * never crosses stdio: `client.callTool` returns an isError RESULT, it does
 * not throw an error carrying a status. So the only rate-limit evidence that
 * reaches this process is what `fail()` put in the text, and there is exactly
 * one producer of it:
 *
 *   api-server/src/auth_middleware.rs:870-874 answers 429 with
 *   `AuthError { error: "Rate limit exceeded", code: "RATE_LIMIT_EXCEEDED",
 *   status: 429 }`, serialised whole as the response body
 *   (auth_middleware.rs:26-34), which core.ts:189-193 folds into the message
 *   `"<METHOD> <path> → 429: {…\"code\":\"RATE_LIMIT_EXCEEDED\"…}"`.
 *
 * That body is NOT the error catalog's shape (it has `code`, not
 * `error_code`), so `parseCatalogError` (core.ts:413-430) returns null and NO
 * `structuredContent` is attached — the text really is the whole signal.
 * `structuredContent.error_code === "rate_limited"` is checked too because
 * the WS surface mints that code (protocol/message_handlers.rs:648); it costs
 * nothing and does not depend on prose.
 *
 * Matching on prose is a compromise and it is stated as one: a typed 429
 * would need `error_catalog.rs` to own the rate-limit refusal, which is a
 * backend change this branch is not permitted to make.
 */
function rateLimitedByWire(result, text) {
  if (result?.structuredContent?.error_code === "rate_limited") return true;
  if (result?.isError !== true || typeof text !== "string") return false;
  return /RATE_LIMIT_EXCEEDED/.test(text) || /→\s*429\b/.test(text);
}

/**
 * Normalise a raw `CallToolResult` into the envelope the rest of this
 * package speaks. PURE — no I/O, no SDK — so tests can feed it results
 * copied verbatim from core.ts / gates.ts / timeline.ts / perception.ts.
 *
 * Fields:
 *   is_error     `res.isError === true` — PRESERVED. Dropping it was
 *                CRITICAL 1: a prose failure parsed to `{raw}` and scored as
 *                an ordinary successful step.
 *   text         the first text content block, verbatim, or null.
 *   data         `JSON.parse(text)` when it yields an object, else null.
 *   parse_error  WHY `data` is null, in words. Never a silent absence.
 *   structured   `res.structuredContent ?? null` — the typed error carrier
 *                (core.ts:498-521), the only place `error_code` /
 *                `retryable` / `details` survive as fields.
 *   refusal      the mirrored gates.ts verdict, `{gate}` or null.
 *   rate_limited see `rateLimitedByWire`.
 */
export function readToolResult(res) {
  const content = Array.isArray(res?.content) ? res.content : [];
  const first = content.find((c) => c?.type === "text" && typeof c.text === "string");
  const text = first ? first.text : null;
  let data = null;
  let parseError = null;
  if (text === null) {
    parseError = "the tool result carried no text content block";
  } else {
    try {
      const parsed = JSON.parse(text);
      if (parsed !== null && typeof parsed === "object") {
        data = parsed;
      } else {
        parseError = `the tool result's text parsed to a bare ${typeof parsed}, not an object`;
      }
    } catch {
      // core.ts:471-497 fail() emits `ERROR: <msg>\nHINT: <hint>` prose, which
      // is not JSON by construction. Saying so is the point: the OLD code
      // wrapped it as `{raw}`, which every downstream reader then treated as a
      // successful result with no fields.
      parseError =
        "the tool result's text is not JSON — core.ts:471-497 fail() emits " +
        "`ERROR: <msg>` / `HINT: <hint>` prose for every failure";
    }
  }
  return {
    is_error: res?.isError === true,
    text,
    data,
    parse_error: parseError,
    structured: res?.structuredContent ?? null,
    refusal: typedRefusalOf(res),
    rate_limited: rateLimitedByWire(res, text),
  };
}

/**
 * The `roshera-mcp` entry point, resolved against THIS MODULE's location and
 * never against the process CWD. `npm run batch --prefix roshera-rl` from the
 * repo root leaves CWD at the repo root, so a CWD-relative default made every
 * spawn fail and every episode SETUP_FAILED (IMPORTANT 7).
 */
export function defaultMcpEntry() {
  return fileURLToPath(new URL("../../roshera-mcp/dist/index.js", import.meta.url));
}

/**
 * Translate the harness's `authHeader` into the child's credential env.
 *
 * The two consumers of `authHeader` want DIFFERENT things and used to
 * disagree silently: episode.mjs/runner.mjs pass it verbatim to `fetch`,
 * while the child reads `ROSHERA_API_KEY` and re-forms the header itself as
 * `Authorization: ApiKey <key>` (core.ts:30-33). Stripping only `^ApiKey `
 * meant a Bearer token was forwarded WHOLE as the key, and every child call
 * 401'd while the harness's own fetches succeeded.
 *
 * `ApiKey <key>` is therefore the only scheme this seam can carry, and
 * anything else is REFUSED loudly at spawn (a SETUP_FAILED naming the reason)
 * rather than converted into a session that 401s on every call.
 */
export function credentialEnv(authHeader) {
  const raw = authHeader?.Authorization;
  if (typeof raw !== "string" || raw.trim() === "") return {};
  const m = /^ApiKey\s+(.+)$/.exec(raw.trim());
  if (!m) {
    throw new Error(
      `authHeader.Authorization uses an unsupported scheme (${raw.split(/\s+/)[0]}): ` +
      `the MCP child reads ROSHERA_API_KEY and sends it as "ApiKey <key>" ` +
      `(roshera-mcp/src/core.ts:30-33), so only an "ApiKey <key>" header can ` +
      `cross this seam. Forwarding it verbatim would 401 every child call ` +
      `while the harness's own fetches kept working`,
    );
  }
  return { ROSHERA_API_KEY: m[1] };
}

/**
 * Resolve a claim binding's `part` reference to an object UUID.
 *
 * `solid:N` is a recipe-local token naming the Nth solid this episode
 * created, mirroring the symbolic-operand convention `recipe_get` already
 * uses for exactly this problem ("body keys named in `symbolic_operands` hold
 * recipe-local tokens ('solid:0') you bind to the ids YOUR re-issue returned"
 * — tools/timeline.ts:220-222). The observed ids come from each create tool's
 * own `object_uuid` field (tools/create.ts:255), which is the only place an
 * object UUID reaches the agent surface: `list_parts` speaks kernel integer
 * ids, and the UUID↔SolidId map lives in the backend's AppState
 * (core.ts:587-598).
 *
 * Returns null when the token names a solid this episode never observed —
 * the caller turns that into a STATED absence, never a silent skip.
 */
function resolveSolidRef(ref, observed) {
  if (typeof ref !== "string" || ref === "") return null;
  const m = /^solid:(\d+)$/.exec(ref);
  if (!m) return ref; // an explicit UUID the task author supplied verbatim
  return observed[Number(m[1])] ?? null;
}

/**
 * Check the task's claims against kernel ground truth.
 *
 * Exported and parameterised by `call` (any `(tool, args) => envelope`) so the
 * logic that decides verified / false / absent is TESTABLE without an SDK. It
 * used to live inside the spawn closure, where the only thing that could
 * exercise it was a fake that replaced it wholesale — the same shape of
 * self-certifying mock this whole fix wave exists to remove.
 *
 * The argument shape is `verify_claim`'s REAL schema
 * (roshera-mcp/src/tools/inspect.ts:63-99): `{expr, bindings:[{var, measure}],
 * expected, tolerance?}` over the five closed measure kinds. The verdict shape
 * is `ClaimVerdict` (geometry-engine/src/readable/claim.rs:64-78):
 * `{verified, refused, computed, expected, abs_error, tolerance_used,
 * resolved, unresolved}` — note `computed`, NOT `measured`.
 *
 * Three outcomes, and the middle one is the whole point: `verified` is
 * `true`/`false` ONLY when the kernel actually measured. When it refused, or
 * the tool call failed, or a binding named a solid this episode never built,
 * the claim is reported ABSENT WITH A STATED REASON and `verified` is null —
 * never a bare null that reads as "checked, and no".
 */
export async function verifyClaims(call, taskClaims, observedSolids) {
  const out = [];
  for (const c of taskClaims) {
    const bindings = [];
    let absent = null;
    for (const b of c.bindings) {
      const m = b.measure;
      if (m.kind === "volume" || m.kind === "surface_area") {
        const part = resolveSolidRef(m.part, observedSolids);
        if (part === null) {
          absent =
            `binding '${b.var}' measures ${m.kind} of ${JSON.stringify(m.part)}, ` +
            `but this episode observed ${observedSolids.length} object_uuid(s) ` +
            `in its tool results, so there is no part to measure`;
          break;
        }
        bindings.push({ var: b.var, measure: { kind: m.kind, part } });
      } else if (m.kind === "face_area") {
        bindings.push({ var: b.var, measure: { kind: "face_area", face: m.face } });
      } else if (m.kind === "edge_length") {
        bindings.push({ var: b.var, measure: { kind: "edge_length", edge: m.edge } });
      } else {
        bindings.push({ var: b.var, measure: { kind: "constant", value: m.value } });
      }
    }
    if (absent !== null) {
      out.push({ name: c.name, verified: null, computed: null, absent });
      continue;
    }
    const env = await call("verify_claim", {
      expr: c.expr, bindings, expected: c.expected, tolerance: c.tolerance,
    });
    const v = env.data;
    const isVerdict =
      v !== null && typeof v?.verified === "boolean" && typeof v?.refused === "boolean";
    if (!isVerdict) {
      // Not a ClaimVerdict: a gate refusal, a transport failure, or prose.
      // Whatever it was, the claim was NOT checked, and that is stated.
      const why = env.refusal
        ? `the call was refused by gate ${JSON.stringify(env.refusal.gate)}`
        : env.is_error
          ? `verify_claim returned an error result: ${env.text}`
          : `verify_claim returned no verdict (${env.parse_error ?? "unrecognised body"})`;
      out.push({ name: c.name, verified: null, computed: null, absent: why });
      continue;
    }
    if (v.refused === true) {
      // claim.rs:116-146 — a binding did not resolve, or the expression did
      // not evaluate to a number. The kernel refuses rather than guessing, and
      // `verified:false` here would misreport a check that never happened as a
      // check that failed.
      out.push({
        name: c.name,
        verified: null,
        computed: null,
        absent:
          `the kernel REFUSED the claim rather than guess: unresolved [` +
          `${(v.unresolved ?? []).join(", ")}]`,
        tolerance_used: v.tolerance_used ?? null,
      });
      continue;
    }
    out.push({
      name: c.name,
      verified: v.verified === true,
      computed: v.computed ?? null,
      expected: v.expected ?? c.expected,
      abs_error: v.abs_error ?? null,
      tolerance_used: v.tolerance_used ?? null,
    });
  }
  return out;
}

/**
 * The episode's recipe — the lineage entry that makes the build RE-ISSUABLE.
 * Exported and `call`-parameterised for the same reason as `verifyClaims`.
 *
 * `recipe_get` returns `{source, step_count, sequence_range,
 * sequence_contiguous, undecodable_events, checkpoints, certificate_summary,
 * reparameterize, steps}` (tools/timeline.ts:267-283 over the api-server's
 * RecipeResponse, handlers/timeline.rs:3986-4008). There is NO `ref` field and
 * there never was — reading one produced a null in every trajectory ever
 * written (CRITICAL 4).
 *
 * TWO facts decide what is recorded here:
 *
 *  1. `reference` MUST be the episode's document id. The route does not read
 *     `X-Roshera-Document` at all (handlers/timeline.rs:4388-4392 has no
 *     document extractor): the default `"main"` resolves to a LIVE BRANCH of
 *     the process-wide ACTIVE document (timeline.rs:4400-4433), which is some
 *     other episode's document or the human's. Addressing the document by id
 *     takes the durable path (timeline.rs:4470-4550), the only one scoped to
 *     THIS episode.
 *  2. The address DIES at reap. `DELETE /api/documents/{id}` deletes the
 *     document's `timeline_events` rows (session-manager/src/database.rs:1704,
 *     3095), so a descriptor alone would be a dangling pointer and the replay
 *     guarantee would stay vacuous. The recipe's own `steps` are therefore
 *     recorded IN the trajectory: they are the re-issuable artifact, and the
 *     trajectory outlives the document.
 */
export async function fetchRecipe(call, documentId) {
  const env = await call("recipe_get", { reference: documentId });
  const r = env.data;
  if (r === null || typeof r?.step_count !== "number") {
    const why = env.refusal
      ? `recipe_get was refused by gate ${JSON.stringify(env.refusal.gate)}` +
        (env.text ? `: ${env.text}` : "")
      : env.is_error
        ? `recipe_get returned an error result: ${env.text}`
        : `recipe_get returned no recipe (${env.parse_error ?? "unrecognised body"})`;
    return { absent: why };
  }
  return {
    retrieved_by: "recipe_get",
    reference: documentId,
    source: r.source ?? null,
    step_count: r.step_count,
    sequence_range: r.sequence_range ?? null,
    sequence_contiguous: r.sequence_contiguous ?? null,
    undecodable_events: r.undecodable_events ?? null,
    checkpoints: r.checkpoints ?? [],
    certificate_summary: r.certificate_summary ?? null,
    steps: r.steps ?? [],
    note:
      "the recipe's STEPS are embedded because the address is not retrievable " +
      "afterwards: the episode's document is deleted at reap and " +
      "DELETE /api/documents purges its timeline_events " +
      "(session-manager/src/database.rs:1704). Replay is recipe-level: " +
      "re-issue these steps, never expect byte-identical geometry.",
    ...(r.step_count === 0
      ? {
          absent:
            "the durable log for this document reported ZERO steps at scoring " +
            "time — either nothing was recorded, or the events were not " +
            "persisted under this document id; this is not evidence that the " +
            "episode built nothing",
        }
      : {}),
  };
}

export async function spawnMcpSession({ documentId, baseUrl, authHeader, mcpEntry }) {
  const credential = credentialEnv(authHeader);
  const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
  const { StdioClientTransport } = await import("@modelcontextprotocol/sdk/client/stdio.js");

  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [mcpEntry ?? defaultMcpEntry()],
    env: {
      ...process.env,
      ROSHERA_DOCUMENT: documentId,
      ROSHERA_URL: baseUrl,
      // AMBIENT PERCEPTION MODE, pinned deliberately. The DEFAULT is
      // `compact` (core.ts:916, 923-926), under which `perception` is a
      // one-line STRING from `compactVerdict` — `perception.sound` does not
      // exist and soundness is unreadable for every step of every episode.
      // `cert` (core.ts:930) returns the full perception OBJECT and, unlike
      // `full`/`xray`, attaches no render image: the boolean this environment
      // scores on, without paying for pixels no policy here looks at.
      ROSHERA_AMBIENT_PERCEPTION: "cert",
      ...credential,
    },
  });
  const client = new Client({ name: "roshera-rl", version: "0.1.0" }, { capabilities: {} });
  await client.connect(transport);

  /** Every `object_uuid` this episode has seen, in the order it appeared. */
  const observedSolids = [];

  /**
   * ONE DISPATCH PATH: every tool call goes through the `invoke` meta-tool.
   *
   * This is not indirection for its own sake — it is the only path that
   * resolves in the DEFAULT surface. `index.ts:96-115` mounts the core+meta
   * surface enabled and every switchable-bench tool DISABLED, so a direct
   * `callTool` for anything outside `CORE_SURFACE` (surface.ts:43-107) does
   * not resolve: `verify_claim` and `recipe_get` are both outside it, which
   * would have made terminal scoring unreachable in exactly the way CRITICAL 3
   * described, with a better-worded absence.
   *
   * `invoke` is the documented long-tail route and is always mounted
   * (META_SURFACE, surface.ts:110-113): it validates against the tool's OWN
   * schema and dispatches to the identical handler — "never less checked or
   * less capable than a direct call" (metatools.ts:538-541) — and returns that
   * handler's result verbatim (metatools.ts:571). Routing EVERY call through
   * it also means a task may allowlist any tool in the table, not only the
   * ~21 resident ones, without the action space depending on residency policy.
   */
  const call = async (tool, args) => {
    const env = readToolResult(
      await client.callTool({ name: "invoke", arguments: { name: tool, args: args ?? {} } }),
    );
    // tools/create.ts:255 — `object_uuid` is what boolean/transform take, and
    // what a volume/surface_area claim binding has to name.
    const uuid = env.data?.object_uuid;
    if (typeof uuid === "string" && uuid !== "" && !observedSolids.includes(uuid)) {
      observedSolids.push(uuid);
    }
    return env;
  };

  return {
    call,
    observedSolids: () => [...observedSolids],
    claims: (taskClaims) => verifyClaims(call, taskClaims, observedSolids),
    recipeRef: () => fetchRecipe(call, documentId),
    async close() { await client.close(); },
  };

}
