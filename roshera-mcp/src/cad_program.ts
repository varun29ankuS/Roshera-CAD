/**
 * Layer 3 — `cad_program` (spec `2026-07-20-mcp-scale-architecture-design.md`,
 * §Layer 3, slice S4). Composition WITH a certificate ledger: run a typed op
 * sequence against the SAME handler implementations individual calls use (via
 * the shared `validateOp` + `entry.handler` dispatch — no duplicated validation
 * or dispatch path), and return a per-op ledger of the certificates each op
 * already produces.
 *
 * Honesty contract (spec §3.3): no rollback, no atomicity pretence.
 *  - All ops are zod-validated UP FRONT; if ANY op is invalid the whole program
 *    is refused with a typed per-op validation report and ZERO ops execute.
 *  - Execution is sequential and STOPS on the first failing op. The ledger names
 *    exactly where it stopped; the real backend state matches the ledger — the
 *    completed-prefix ops are applied, the rest were never attempted.
 *
 * Footgun guards (spec §S4.3): ops may not be meta-tools (no recursion through
 * find_tool/describe_tool/invoke/workbench/cad_program) and may not be the
 * destructive clear_parts/delete_part unless the program sets allow_destructive.
 */

import { z } from "zod";
import { ToolHost, ToolTable } from "./registry.js";
import { validateOp, UnknownToolError, rankTools } from "./metatools.js";
import { ok } from "./core.js";
import { McpError } from "@modelcontextprotocol/sdk/types.js";

/** Max ops per program (spec S4.1, slice-1 cap). */
export const MAX_OPS = 50;

/** Meta-tools may never appear as a program op (no recursion / no funnel-in-batch). */
const META_OPS = new Set<string>([
  "find_tool",
  "describe_tool",
  "invoke",
  "workbench",
  "cad_program",
]);

/** Destructive ops gated behind an explicit allow_destructive flag. */
const DESTRUCTIVE_OPS = new Set<string>(["clear_parts", "delete_part"]);

interface ValidationIssue {
  index: number;
  tool: string;
  reason: string;
}

// ─── Output→input chaining (2026-08-09) ─────────────────────────────────────
//
// Args used to be LITERAL: no result of one op could reach a later op's args,
// which forced every id-returning call (psketch_begin) OUTSIDE the program and
// split the canonical begin→polyline→extrude flow across MCP turns. The
// minimal honest mechanism: a string arg that is EXACTLY `$N.<dot.path>` or
// `$prev.<dot.path>` (whole string — never interpolated inside a longer one)
// is replaced, just before the op runs, by that field of op N's (or the
// immediately preceding op's) parsed JSON result. What stays honest:
//   - references are validated UP FRONT (forward/self references refuse the
//     whole program with zero execution, same as any bad op);
//   - schema validation of a placeholder-carrying op MOVES to execution time,
//     right after substitution — it cannot run earlier because the value does
//     not exist yet. The tool description says so; every other op keeps the
//     up-front guarantee unchanged;
//   - an unresolvable path (op returned no JSON / key absent) is a typed
//     ledger error naming the placeholder AND the keys that were available,
//     and stops the program there — no guessing, no partial substitution;
//   - `$$` at the start of a string escapes a literal `$` (applied uniformly
//     to every op's string args, placeholder-carrying or not).

/** Strict whole-string placeholder: `$prev.<path>` or `$<index>.<path>`. */
const PLACEHOLDER_RE = /^\$(\d+|prev)\.([A-Za-z0-9_][A-Za-z0-9_.]*)$/;

interface PlaceholderRef {
  /** The verbatim placeholder string, for error messages. */
  token: string;
  /** Referenced op index, or "prev" (resolved to i-1 at execution). */
  target: number | "prev";
}

/** Collect every placeholder string anywhere in a raw args value. */
function collectPlaceholders(v: unknown, out: PlaceholderRef[]): void {
  if (typeof v === "string") {
    const m = PLACEHOLDER_RE.exec(v);
    if (m) out.push({ token: v, target: m[1] === "prev" ? "prev" : Number(m[1]) });
    return;
  }
  if (Array.isArray(v)) {
    for (const x of v) collectPlaceholders(x, out);
    return;
  }
  if (v && typeof v === "object") {
    for (const x of Object.values(v)) collectPlaceholders(x, out);
  }
}

/** Thrown by substitutePlaceholders when a reference cannot be resolved. */
class ChainResolutionError extends Error {}

/**
 * Deep-copy `v` with every placeholder string replaced by the referenced
 * field of an earlier op's result and every `$$`-prefixed string unescaped.
 * `results[t]` is op t's parsed JSON result (null when it returned no JSON).
 * Throws ChainResolutionError with an exact, actionable message on any miss.
 * With no placeholders present (results ignored) this is a pure unescape
 * pass, which is how phase 1 applies the `$$` rule uniformly.
 */
function substitutePlaceholders(
  v: unknown,
  opIndex: number,
  results: (unknown | null)[],
): unknown {
  if (typeof v === "string") {
    if (v.startsWith("$$")) return v.slice(1); // escaped literal `$…`
    const m = PLACEHOLDER_RE.exec(v);
    if (!m) return v;
    const target = m[1] === "prev" ? opIndex - 1 : Number(m[1]);
    let cur: unknown = results[target];
    if (cur === null || cur === undefined) {
      throw new ChainResolutionError(
        `${v}: op ${target} returned no JSON object to chain from`,
      );
    }
    for (const key of m[2].split(".")) {
      if (cur === null || typeof cur !== "object" || !(key in (cur as object))) {
        const keys =
          cur !== null && typeof cur === "object"
            ? Object.keys(cur as object)
            : [];
        throw new ChainResolutionError(
          `${v}: op ${target}'s result has no '${key}'` +
            (keys.length ? ` (available: ${keys.join(", ")})` : ""),
        );
      }
      cur = (cur as Record<string, unknown>)[key];
    }
    return cur;
  }
  if (Array.isArray(v)) {
    return v.map((x) => substitutePlaceholders(x, opIndex, results));
  }
  if (v && typeof v === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) {
      out[k] = substitutePlaceholders(x, opIndex, results);
    }
    return out;
  }
  return v;
}

/**
 * The parsed JSON of a successful op result's first JSON text block — the
 * source side of chaining. Null when the op returned no JSON (image-only
 * results): chaining from such an op fails typed, never silently.
 */
function firstJsonOf(result: any): unknown | null {
  const content: any[] = Array.isArray(result?.content) ? result.content : [];
  for (const c of content) {
    if (c?.type === "text" && typeof c.text === "string") {
      try {
        const data = JSON.parse(c.text);
        if (data && typeof data === "object") return data;
      } catch {
        // not JSON — keep looking
      }
    }
  }
  return null;
}

interface LedgerEntry {
  index: number;
  tool: string;
  ok: boolean;
  certificate?: unknown;
  summary?: string;
  error?: string;
}

/**
 * Extract the certificate (perception/soundness block) an op's own result
 * already carries. Every mutating tool returns `{content:[{type:"text",
 * text: JSON}]}` whose JSON carries a `perception` field (the "SOUND ✓ …" verdict
 * line or object) — that IS the certificate, byte-for-byte what a single call
 * produced. Tools that return no perception (e.g. render_part → image content)
 * get a short summary instead. Never fabricates a verdict.
 */
function extractCertificate(result: any): {
  certificate?: unknown;
  summary?: string;
} {
  const content: any[] = Array.isArray(result?.content) ? result.content : [];
  const textBlocks = content
    .filter((c) => c?.type === "text" && typeof c.text === "string")
    .map((c) => c.text as string);

  for (const t of textBlocks) {
    try {
      const data = JSON.parse(t);
      if (data && typeof data === "object" && "perception" in data) {
        return { certificate: (data as any).perception };
      }
    } catch {
      // not JSON — fall through to summary
    }
  }

  // No perception block — record ok + a compact summary of what came back.
  const hasImage = content.some((c) => c?.type === "image");
  for (const t of textBlocks) {
    try {
      const data = JSON.parse(t);
      if (data && typeof data === "object") {
        const keys = Object.keys(data);
        return {
          summary:
            `ok — returned {${keys.join(", ")}}` +
            (hasImage ? " + image content" : "") +
            " (no soundness certificate on this op)",
        };
      }
    } catch {
      // non-JSON text
    }
  }
  if (hasImage) {
    return { summary: "ok — image content (no soundness certificate on this op)" };
  }
  const firstText = textBlocks[0];
  return {
    summary: firstText
      ? `ok — ${firstText.slice(0, 120)}`
      : "ok — no textual result",
  };
}

/** First text content block of a failed op result (the "ERROR: …" message). */
function errorTextOf(result: any): string {
  const content: any[] = Array.isArray(result?.content) ? result.content : [];
  const t = content.find(
    (c) => c?.type === "text" && typeof c.text === "string",
  );
  return t ? String(t.text) : "op failed with no error message";
}

export function registerCadProgram(host: ToolHost, table: ToolTable): void {
  host.tool(
    "cad_program",
    "Run up to 50 tool ops as ONE certified program through the SAME handlers " +
      "individual calls use. ALL ops are schema-validated up front — any bad op " +
      "refuses the WHOLE program (per-op report, nothing runs). Execution is " +
      "sequential and STOPS at the first failure, returning a LEDGER {completed, " +
      "total, ops:[{index, tool, ok, certificate|error}]} — the certificate is " +
      "each op's own soundness verdict. NO rollback: backend state = the " +
      "completed prefix exactly; undo is your explicit next call. Ops may not be " +
      "meta/composition tools, nor clear_parts/delete_part unless " +
      "allow_destructive is set. A many-vertex profile is ONE polyline op, " +
      "never one op per vertex (single-point runs past 8 refuse typed). " +
      "CHAINING: a string arg exactly '$N.key' or '$prev.key' (dot-path into " +
      "an earlier op's result) resolves before the op runs — begin, polyline " +
      "{csketch_id:'$0.csketch_id'}, extrude is ONE program. Placeholder ops " +
      "validate at run time; an unresolvable path or forward reference is a " +
      "typed refusal; '$$' escapes a literal '$'.",
    {
      name: z
        .string()
        .optional()
        .describe("optional label for the program (echoed in the ledger)"),
      ops: z
        .array(
          z.object({
            tool: z.string().min(1).describe("exact tool name (from find_tool)"),
            args: z
              .record(z.any())
              .optional()
              .describe("the tool's arguments (validated by its own schema)"),
          }),
        )
        .min(1)
        .max(MAX_OPS)
        .describe(`ordered ops to run (1..${MAX_OPS})`),
      allow_destructive: z
        .boolean()
        .optional()
        .describe(
          "permit clear_parts/delete_part ops (footgun guard; default false)",
        ),
    },
    async ({ name, ops, allow_destructive }, extra) => {
      const total = ops.length;
      const allowDestructive = allow_destructive === true;

      // ── PHASE 1: validate EVERY op up front (cheap honesty) ────────────────
      // Resolve + zod-validate through each tool's own schema (the identical
      // validateOp path invoke runs) and apply the meta/destructive guards.
      // Any issue fails the whole program with ZERO execution.
      const issues: ValidationIssue[] = [];
      const parsedOps: {
        tool: string;
        parsed: any;
        /** Raw args of a placeholder-carrying op — substituted, THEN schema-
         *  validated, at execution time (the value does not exist earlier). */
        deferred?: { raw: any };
      }[] = [];
      for (let i = 0; i < ops.length; i++) {
        const { tool, args } = ops[i];
        if (META_OPS.has(tool)) {
          issues.push({
            index: i,
            tool,
            reason:
              `'${tool}' is a meta/composition tool and cannot be a program op ` +
              "(no recursion; call the concrete tools directly).",
          });
          parsedOps.push({ tool, parsed: undefined });
          continue;
        }
        if (DESTRUCTIVE_OPS.has(tool) && !allowDestructive) {
          issues.push({
            index: i,
            tool,
            reason:
              `'${tool}' is destructive; set allow_destructive: true on the ` +
              "program to permit it.",
          });
          parsedOps.push({ tool, parsed: undefined });
          continue;
        }
        // Chaining: an op whose args carry placeholders cannot be schema-
        // validated before its inputs exist. Its REFERENCES are validated
        // here (tool must exist; targets must be earlier ops); its schema
        // validation moves to execution time, right after substitution.
        const refs: PlaceholderRef[] = [];
        collectPlaceholders(args ?? {}, refs);
        if (refs.length > 0) {
          if (!table.has(tool)) {
            const near = rankTools(table, tool, undefined, 5).map((r) => r.name);
            issues.push({
              index: i,
              tool,
              reason:
                `unknown tool '${tool}'` +
                (near.length ? ` (did you mean: ${near.join(", ")}?)` : ""),
            });
            parsedOps.push({ tool, parsed: undefined });
            continue;
          }
          const bad = refs.find((r) =>
            r.target === "prev" ? i === 0 : r.target >= i,
          );
          if (bad) {
            issues.push({
              index: i,
              tool,
              reason:
                `'${bad.token}' references ` +
                (bad.target === "prev"
                  ? "the previous op, but this is the first op"
                  : `op ${bad.target}, which does not run before op ${i}`) +
                " — a placeholder may only read an EARLIER op's result.",
            });
            parsedOps.push({ tool, parsed: undefined });
            continue;
          }
          parsedOps.push({ tool, parsed: undefined, deferred: { raw: args ?? {} } });
          continue;
        }
        try {
          // No placeholders: full up-front schema validation, on the args
          // with the uniform `$$`→`$` unescape applied (a pure transform —
          // with no placeholders present, results are never consulted).
          const literal = substitutePlaceholders(args ?? {}, i, []);
          const { parsed } = await validateOp(table, tool, literal);
          parsedOps.push({ tool, parsed });
        } catch (e) {
          const reason =
            e instanceof UnknownToolError
              ? `unknown tool '${tool}'` +
                (e.nearest.length ? ` (did you mean: ${e.nearest.join(", ")}?)` : "")
              : e instanceof McpError
                ? e.message
                : e instanceof Error
                  ? e.message
                  : String(e);
          issues.push({ index: i, tool, reason });
          parsedOps.push({ tool, parsed: undefined });
        }
      }

      if (issues.length > 0) {
        const report = ok({
          ok: false,
          stage: "validation",
          name: name ?? null,
          total,
          executed: 0,
          errors: issues,
          note:
            "No ops were executed — validation failed up front (every op is " +
            "checked before any runs, so a bad batch costs nothing). Fix the " +
            "listed ops and resubmit.",
        });
        (report as any).isError = true;
        return report;
      }

      // ── PHASE 2: execute sequentially, STOP on the first failure ───────────
      const ledger: LedgerEntry[] = [];
      /** Parsed JSON result of each completed op — the chaining source. */
      const chainResults: (unknown | null)[] = [];
      let completed = 0;
      let stoppedAt: number | null = null;
      for (let i = 0; i < parsedOps.length; i++) {
        const { tool, deferred } = parsedOps[i];
        // present — phase 1 resolved it (validateOp, or table.has for deferred)
        const entry = table.get(tool)!;
        let parsed = parsedOps[i].parsed;
        if (deferred) {
          // Resolve placeholders against the completed prefix, then run the
          // SAME validateOp a direct call runs — deferred, not skipped.
          let substituted: unknown;
          try {
            substituted = substitutePlaceholders(deferred.raw, i, chainResults);
          } catch (e) {
            ledger.push({
              index: i,
              tool,
              ok: false,
              error:
                `placeholder resolution failed: ` +
                (e instanceof Error ? e.message : String(e)) +
                `. The ${i} op(s) before this ran and their state is live.`,
            });
            stoppedAt = i;
            break;
          }
          try {
            parsed = (await validateOp(table, tool, substituted)).parsed;
          } catch (e) {
            ledger.push({
              index: i,
              tool,
              ok: false,
              error:
                (e instanceof Error ? e.message : String(e)) +
                " (schema-validated at execution time, after placeholder resolution)",
            });
            stoppedAt = i;
            break;
          }
        }
        let result: any;
        try {
          // DISPATCH PARITY: the same handler a direct/invoke call runs.
          result = await entry.handler(parsed, extra);
        } catch (e) {
          // A handler that throws (rather than returning a typed fail) — record
          // and stop; the state is whatever the throwing op left behind.
          ledger.push({
            index: i,
            tool,
            ok: false,
            error: e instanceof Error ? e.message : String(e),
          });
          stoppedAt = i;
          break;
        }
        if (result?.isError === true) {
          // Typed backend refusal / timeout / network error surfaced by the
          // handler as an error result — stop here (stop-on-first-error).
          ledger.push({ index: i, tool, ok: false, error: errorTextOf(result) });
          stoppedAt = i;
          break;
        }
        const cert = extractCertificate(result);
        ledger.push({ index: i, tool, ok: true, ...cert });
        chainResults[i] = firstJsonOf(result);
        completed += 1;
      }

      const allOk = stoppedAt === null;
      const note = allOk
        ? "All ops completed; each ledger entry carries the op's own certificate. " +
          "State matches the ledger."
        : `Stopped at op ${stoppedAt} (${ledger[ledger.length - 1]?.tool}). No rollback: ` +
          `the first ${completed} op(s) are applied and live; ops after the stop were ` +
          "never attempted. State matches the ledger exactly — undo/truncate is your " +
          "explicit next call.";

      return ok({
        ok: allOk,
        name: name ?? null,
        completed,
        total,
        stopped_at: stoppedAt,
        ops: ledger,
        note,
      });
    },
  );
}
