/**
 * The agent's HANDS on a sheet (2026-11-20, agent eyes slice 3).
 *
 * Each tool is one layout verb on POST /api/drawings/{id}/layout (api-server
 * drawing_hands.rs; the kernel plans it in geometry_engine::drawing::hands):
 * move a dimension to a band or lane, align a row, move a view, re-route a
 * leader, remove a view that adds nothing, remove a dimension stated
 * elsewhere. The server gates every verb -- refused, with the sheet
 * unchanged, when it has no room, when the sheet would go unsound or stop
 * stating a value, or when it would add a layout error -- records it as one
 * undoable `drawing.layout.update` event, and answers what it did to the
 * space map: each touched space's score before and after (worst first), the
 * grade and below-bar counts, the issues created and resolved, and the
 * certificate. The call carries X-Roshera-Agent, so the route also shows the
 * act and the judgement in the person's open app.
 */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ApiError, fail } from "../core.js";

/** How many changed spaces the text answer lists (all ride structuredContent). */
const SHOWN_SPACES = 10;
/** How many created / resolved issues the text answer lists. */
const SHOWN_ISSUES = 8;

const round3 = (v: unknown) => (typeof v === "number" ? +v.toFixed(3) : v ?? null);

/** One issue as a line: rule, space, message. */
function issueLine(i: any): string {
  return `${i?.severity === "error" ? "ERROR" : "warn"} ${i?.rule} in ${i?.space}: ${i?.message}`;
}

function capped(lines: string[], n: number): string[] {
  return lines.length > n ? [...lines.slice(0, n), `+${lines.length - n} more`] : lines;
}

/**
 * POST one verb and answer compactly: what was done and why, the score
 * change of every touched space (worst first), the grade and below-bar
 * change, the issues created and resolved, the certificate. A refusal
 * (409 layout_verb_refused) comes back through `fail`, its sentence as the
 * top-level `reason` and its code in `details.reason`.
 */
async function hand(drawingId: string, body: Record<string, unknown>) {
  try {
    const r = await api("POST", `/api/drawings/${drawingId}/layout`, body, 180_000);
    const changed: any[] = Array.isArray(r?.spaces_changed) ? r.spaces_changed : [];
    const created: any[] = Array.isArray(r?.issues_created) ? r.issues_created : [];
    const resolved: any[] = Array.isArray(r?.issues_resolved) ? r.issues_resolved : [];
    const compact = {
      drawing_id: drawingId,
      verb: r?.verb ?? body.verb,
      done: r?.caption ?? r?.done ?? null,
      grade: `${round3(r?.grade_before)} -> ${round3(r?.grade_after)}`,
      below_bar: `${r?.below_bar_before ?? "?"} -> ${r?.below_bar_after ?? "?"}`,
      spaces_changed: changed.slice(0, SHOWN_SPACES).map((c) => ({
        space: c?.space,
        before: round3(c?.before),
        after: round3(c?.after),
        better: typeof c?.before === "number" && typeof c?.after === "number" ? c.after > c.before : null,
      })),
      more_spaces_changed: Math.max(0, changed.length - SHOWN_SPACES),
      issues_created: capped(created.map(issueLine), SHOWN_ISSUES),
      issues_resolved: capped(resolved.map(issueLine), SHOWN_ISSUES),
      certificate: {
        sound: r?.certificate?.sound ?? null,
        complete: r?.certificate?.complete ?? null,
      },
      shown_in_app: r?.shown_to ?? null,
      note:
        created.length > 0
          ? "this verb created issues: look at the spaces it touched (drawing_look) before going on"
          : "undo with timeline_undo; the whole answer is in structuredContent",
    };
    return {
      content: [{ type: "text" as const, text: JSON.stringify(compact, null, 2) }],
      structuredContent: { ...compact, response: r },
    };
  } catch (e) {
    return refused(e);
  }
}

/**
 * A refused verb: `fail`'s typed result with the SERVER's sentence as the
 * top-level `reason` (what a refusal card shows, and what the agent quotes)
 * instead of the transport line around it, and the stable code beside it as
 * `refusal` (`no_room`, `drops_value`, `unsound`, `layout_error`, ...).
 */
function refused(e: unknown) {
  const out = fail(e);
  if (e instanceof ApiError && out.structuredContent) {
    try {
      const body = JSON.parse(e.body);
      if (typeof body?.error === "string" && body.error.trim()) {
        out.structuredContent.reason = body.error;
      }
      if (typeof body?.details?.reason === "string") {
        out.structuredContent.refusal = body.details.reason;
      }
    } catch {
      // Not a JSON body: fail()'s own reason stands.
    }
  }
  return out;
}

const drawingId = z.string().uuid().describe("drawing_id from make_drawing");

export function registerDrawingHandsTools(server: ToolHost): void {
  // ── drawing_move_dimension ─────────────────────────────────────────────
  server.tool(
    "drawing_move_dimension",
    "MOVE ONE SHEET DIMENSION: to another band of its view (top, bottom, left, " +
      "right) or lane (1 = nearest the outline), or slide its text along its " +
      "line (offset_mm). A horizontal dimension runs in the top or bottom band, " +
      "a vertical one left or right; a leader callout's shoulder goes left or " +
      "right. Refused when that place has no room. Recorded, undoable; answers " +
      "each touched space's score before and after.",
    {
      drawing_id: drawingId,
      dimension: z.string().describe("dimension id from drawing_spaces / drawing_dimensions"),
      band: z.enum(["top", "bottom", "left", "right"]).optional().describe("the band of its view"),
      lane: z.number().int().min(1).optional().describe("the lane of the band, 1 = nearest the outline"),
      offset_mm: z.number().optional().describe("slide the text along its line, mm (right or down positive)"),
    },
    async ({ drawing_id, dimension, band, lane, offset_mm }) =>
      hand(drawing_id, { verb: "move_dimension", dimension, band, lane, offset_mm }),
  );

  // ── drawing_align ──────────────────────────────────────────────────────
  server.tool(
    "drawing_align",
    "LINE UP DIMENSIONS: put parallel dimensions of one view on one line (one " +
      "lane of one band) -- the lane nearest the outline that holds them all. " +
      "Refused when two overlap along the line. Recorded, undoable; answers the " +
      "score change.",
    {
      drawing_id: drawingId,
      dimensions: z.array(z.string()).min(2).describe("dimension ids to put on one line"),
    },
    async ({ drawing_id, dimensions }) => hand(drawing_id, { verb: "align", dimensions }),
  );

  // ── drawing_move_view ──────────────────────────────────────────────────
  server.tool(
    "drawing_move_view",
    "MOVE A VIEW ON ITS SHEET: its geometry's centre to a point (`to`, paper " +
      "mm, y down, as drawing_spaces boxes read), or onto the projection line " +
      "of another view (`align_with`). Projected views stay on their lines and " +
      "FRONT carries them; a drop on other ink is refused. Recorded, undoable.",
    {
      drawing_id: drawingId,
      view: z.string().describe("view name (FRONT) or space id (view:FRONT)"),
      to: z
        .array(z.number())
        .length(2)
        .optional()
        .describe("where the view's geometry centre goes, [x, y] paper mm, y down"),
      align_with: z.string().optional().describe("a view to line it up with"),
    },
    async ({ drawing_id, view, to, align_with }) =>
      hand(drawing_id, { verb: "move_view", view, to, align_with }),
  );

  // ── drawing_reattach_leader ────────────────────────────────────────────
  server.tool(
    "drawing_reattach_leader",
    "RE-ROUTE A LEADER: short, its leg at 30-60 degrees, its text or frame " +
      "clear of the views and crossing no line. A dimension's leader callout " +
      "(its id) or a feature control frame ('fcf:<n>' from drawing_spaces); " +
      "target 'nearest' ends a frame's leader on its feature's edge instead of " +
      "its centre. Refused when no clear route exists. Recorded, undoable.",
    {
      drawing_id: drawingId,
      dimension: z.string().describe("a dimension id on a leader, or a frame 'fcf:<n>' from drawing_spaces"),
      side: z.enum(["left", "right"]).optional().describe("which side of its feature the text or frame goes"),
      target: z.enum(["nearest"]).optional().describe("frames: end the leader on the nearest edge of its feature"),
    },
    async ({ drawing_id, dimension, side, target }) =>
      hand(drawing_id, { verb: "reattach_leader", dimension, side, target }),
  );

  // ── drawing_remove_view ────────────────────────────────────────────────
  server.tool(
    "drawing_remove_view",
    "REMOVE A VIEW THAT ADDS NOTHING (the RIGHT view of a round part repeats " +
      "FRONT). Refused, naming them, when a dimension would then be stated on " +
      "no view, or when the view carries the section, datums or frames. " +
      "Recorded, undoable.",
    {
      drawing_id: drawingId,
      view: z.string().describe("view name (RIGHT) or space id (view:RIGHT)"),
    },
    async ({ drawing_id, view }) => hand(drawing_id, { verb: "remove_view", view }),
  );

  // ── drawing_remove_dimension ───────────────────────────────────────────
  server.tool(
    "drawing_remove_dimension",
    "REMOVE A DIMENSION THE SHEET STATES ELSEWHERE, as a signed, recorded " +
      "suppression. Refused when the sheet would no longer state its value " +
      "(a waiver of a needed value is the engineer's: drawing_dimension_properties).",
    {
      drawing_id: drawingId,
      dimension: z.string().describe("dimension id from drawing_dimensions"),
      by: z.string().describe("who removes it"),
      reason: z.string().optional().describe("why"),
    },
    async ({ drawing_id, dimension, by, reason }) =>
      hand(drawing_id, { verb: "remove_dimension", dimension, by, reason: reason ?? "" }),
  );
}
