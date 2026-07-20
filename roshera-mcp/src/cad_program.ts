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
import { validateOp, UnknownToolError } from "./metatools.js";
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
    "Run a sequence of tool ops (max 50) as ONE certified program against the " +
      "SAME handlers individual calls use. All ops are validated up front — one " +
      "bad op refuses the whole program with a per-op validation report and runs " +
      "NOTHING. Otherwise ops run sequentially and STOP on the first failure, " +
      "returning a LEDGER: {completed, total, ops:[{index, tool, ok, certificate|" +
      "error}]} — the certificate is the soundness verdict each op already emits. " +
      "No rollback and no atomicity pretence: the backend state matches the ledger " +
      "exactly (the completed prefix is applied; undo/truncate is your explicit " +
      "next call). Ops may not be meta-tools (find_tool/describe_tool/invoke/" +
      "workbench/cad_program) and may not be clear_parts/delete_part unless " +
      "allow_destructive is set.",
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
      const parsedOps: { tool: string; parsed: any }[] = [];
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
        try {
          const { parsed } = await validateOp(table, tool, args ?? {});
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
      let completed = 0;
      let stoppedAt: number | null = null;
      for (let i = 0; i < parsedOps.length; i++) {
        const { tool, parsed } = parsedOps[i];
        const entry = table.get(tool)!; // present — validateOp resolved it in phase 1
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
