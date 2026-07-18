/** Timeline tools — event-sourced history: scrub, clear. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { randomUUID } from "node:crypto";
import { api, ok, fail, ApiError } from "../core.js";

/**
 * A mould / bind endpoint returns a TYPED refusal (409/422/404) when the edit
 * is not honourable — a broken-downstream feature, an unknown parameter name,
 * an unbindable target. That is an honest ANSWER, not a tool failure: surface
 * the parsed verdict as an `ok()` result so the agent sees exactly why the
 * edit was refused (never a silent bad model). Genuine transport errors still
 * fall through to `fail()`.
 */
function refusalOrFail(e: unknown) {
  if (e instanceof ApiError && [404, 409, 422].includes(e.status)) {
    try {
      return ok({ refused: JSON.parse(e.body) });
    } catch {
      /* body not JSON — fall through */
    }
  }
  return fail(e);
}

export function registerTimelineTools(server: McpServer) {
  server.tool(
    "timeline_mould",
    "Edit a recorded parameter and re-derive the model (#64 parametric DAG). " +
      "The edit is APPENDED as a `param.mould` override event and the branch " +
      "is full-replayed with it folded in — the original event is never " +
      "mutated (append-only). Downstream features re-derive; PID references " +
      "survive a dimensional edit. An edit that breaks a downstream feature is " +
      "REFUSED with a typed verdict (never a silent bad model). Target by " +
      "`target_event_id`+`parameter` (raw key like 'radius'/'width'), or by a " +
      "stable `name` bound via bind_parameter_name.",
    {
      value: z.number().describe("the new dimensional value"),
      target_event_id: z
        .string()
        .optional()
        .describe("event UUID to edit (with `parameter`)"),
      parameter: z
        .string()
        .optional()
        .describe("raw numeric parameter key on the target event, e.g. 'radius'"),
      name: z
        .string()
        .optional()
        .describe("stable parameter name to target (see bind_parameter_name)"),
      branch: z.string().default("main"),
    },
    async ({ value, target_event_id, parameter, name, branch }) => {
      try {
        // #29 — address the branch's LIVE session directly (no session_id):
        // the mould reconciles the live/active model on `branch` the same way
        // dependency-graph/{branch} and rebuild-certificate/{branch} address it.
        // A part built purely through the live geometry tools is mouldable
        // end-to-end without discovering a session UUID.
        const r = await api("POST", "/api/timeline/mould", {
          branch_id: branch,
          target_event_id,
          parameter,
          name,
          value,
        });
        return ok(r);
      } catch (e) {
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "bind_parameter_name",
    "Bind a stable, agent-friendly NAME to a recorded (event, parameter) so a " +
      "mould can target it by name (#64 Slice 3). Appended `param.name` event " +
      "(append-only, latest-binding-wins). The parameter must be a numeric " +
      "dimension of the target event, else the bind is refused.",
    {
      name: z.string().describe("the name to bind, e.g. 'bore_diameter'"),
      target_event_id: z.string().describe("event UUID whose parameter to name"),
      parameter: z.string().describe("raw numeric parameter key, e.g. 'radius'"),
      branch: z.string().default("main"),
    },
    async ({ name, target_event_id, parameter, branch }) => {
      try {
        const r = await api("POST", "/api/timeline/parameter-name", {
          branch_id: branch,
          name,
          target_event_id,
          parameter,
        });
        return ok(r);
      } catch (e) {
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "rebuild_certificate",
    "The honest per-feature rebuild certificate for a branch's CURRENT state " +
      "(#64 Slice 5). For every feature: Rebuilt / Unaffected / Failed{reason} / " +
      "Dangling{entity} (a reference that no longer resolves) / Blocked{by} " +
      "(downstream of a break), plus the dirty sequences and a re-measured " +
      "`is_sound` recomputed from the B-Rep (never asserted). Use after a mould " +
      "to see exactly what the edit did to every dependent.",
    { branch: z.string().default("main") },
    async ({ branch }) => {
      try {
        const r = await api(
          "GET",
          `/api/timeline/rebuild-certificate/${branch}`,
        );
        return ok(r);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "timeline_scrub",
    "Look at the scene AS OF a past event — non-destructive (live scene " +
      "untouched). Returns object count + mesh stats at that moment.",
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
    "Reset a timeline branch to ZERO events and wipe the live model to match " +
      "— DESTRUCTIVE and irreversible (the event ledger is rewritten, not " +
      "undoable). Use clear_parts instead for an empty scene with preserved " +
      "history.",
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
}
