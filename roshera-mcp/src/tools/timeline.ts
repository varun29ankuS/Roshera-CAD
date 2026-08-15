/** Timeline tools — event-sourced history: the agent's memory (history,
 * checkpoints, undo/redo) and its parallel-work substrate (branch,
 * merge, conflicts), plus scrub, mould, clear. */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { randomUUID } from "node:crypto";
import { api, ok, fail, ApiError } from "../core.js";
import { gate6WouldRefuse } from "../gates.js";

/**
 * One stable session id per MCP process: the backend's undo/redo walk a
 * per-session cursor (seeded at the current head on first use), so the
 * id must persist across calls or every undo would target the same last
 * event. Reconnecting the MCP starts a fresh cursor at the new head.
 */
const AGENT_SESSION_ID = randomUUID();

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

export function registerTimelineTools(server: ToolHost) {
  server.tool(
    "timeline_mould",
    "Edit a recorded parameter and re-derive the model (#64 parametric DAG). The " +
      "edit is APPENDED as a param.mould override; the branch re-derives (append-" +
      "only; PIDs survive). An edit that breaks a downstream feature is REFUSED " +
      "with a typed verdict. Target by target_event_id+parameter, or a `name` " +
      "bound via bind_parameter_name.",
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
      branch: z.string().default("main").describe("timeline branch id ('main' = trunk)"),
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
    "Bind a stable NAME to a recorded (event, parameter) so a mould can target " +
      "it by name. Appended, latest-binding-wins. The parameter must be a " +
      "numeric dimension of the target event, else refused.",
    {
      name: z.string().describe("the name to bind, e.g. 'bore_diameter'"),
      target_event_id: z.string().describe("event UUID whose parameter to name"),
      parameter: z.string().describe("raw numeric parameter key, e.g. 'radius'"),
      branch: z.string().default("main").describe("timeline branch id ('main' = trunk)"),
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
    "Per-feature rebuild certificate for a branch's CURRENT state: each feature " +
      "Rebuilt / Unaffected / Failed / Dangling / Blocked, plus dirty sequences " +
      "and a re-measured `is_sound`. Use after a mould to see what the edit did.",
    { branch: z.string().default("main").describe("timeline branch id ('main' = trunk)") },
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
    {
      branch: z.string().default("main").describe("timeline branch id ('main' = trunk)"),
      sequence: z.number().int().describe("event sequence number to view the scene as-of"),
    },
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
    "timeline_history",
    "Read a branch's recorded design history — the timeline is the agent's " +
      "queryable memory. Events in sequence order (id, kind, author, affected " +
      "parts); page with start/limit. include_operations adds each event's " +
      "full recorded parameters.",
    {
      branch: z.string().default("main").describe("timeline branch id ('main' = trunk)"),
      start: z.number().int().min(0).default(0).describe("first event sequence number to return"),
      limit: z.number().int().min(1).max(1000).default(100).describe("max events to return"),
      include_operations: z
        .boolean()
        .default(false)
        .describe("include each event's full recorded operation payload"),
    },
    async ({ branch, start, limit, include_operations }) => {
      try {
        const r = await api(
          "GET",
          `/api/timeline/history/${encodeURIComponent(branch)}?start=${start}&limit=${limit}`,
        );
        // The backend serves a bare array for an ordinary document, and
        // {events, durability} on a QUARANTINED one — the served events are
        // only the clean prefix of the persisted log; the tail is refused,
        // never silently dropped. A bare `Array.isArray(r) ? r : []` here
        // would turn that disclosure into an apparent empty history — the
        // exact "an agent asks what happened and gets a clean answer" defect
        // one layer up. Handle both shapes explicitly.
        const raw: any[] = Array.isArray(r) ? r : Array.isArray(r?.events) ? r.events : [];
        const events = raw.map((e: any) => ({
          id: e.id,
          sequence: e.sequence_number,
          timestamp: e.timestamp,
          operation_type: e.operation_type,
          author: e.author,
          author_kind: e.author_kind,
          affected_parts: e.affected_parts,
          ...(include_operations ? { operation: e.operation } : {}),
        }));
        const durability = !Array.isArray(r) ? (r?.durability ?? null) : null;
        return ok({
          branch,
          start,
          count: events.length,
          events,
          ...(durability ? { durability } : {}),
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "recipe_get",
    "RETRIEVE A PROVEN PLAN INSTEAD OF INVENTING ONE. Projects a certified " +
      "build into a re-parameterizable RECIPE: its ordered op kinds, the " +
      "parameters AS RECORDED, the intent recorded on each op, the checkpoint " +
      "declarations covering them, and a roll-up of the certificates AS RECORDED. " +
      "Addressable by branch ('main') OR by any document id — a document that is " +
      "not open is read from durable storage and is NOT opened, so retrieving a " +
      "recipe never disturbs what you are working on. THE PATTERN: for a component you " +
      "have built before (flange, gear, housing), find the document that built it, pull its " +
      "recipe, EDIT THE NUMBERS in each op's `reissue.body`, and re-issue the ops " +
      "in order via cad_program — do not design such a component from scratch. " +
      "Every op carries `reissue` (the route + body that re-issues it) or an " +
      "explicit `reissue_absent_reason`; body keys named in `symbolic_operands` " +
      "hold recipe-local tokens ('solid:0') you bind to the ids YOUR re-issue " +
      "returned.",
    {
      reference: z
        .string()
        .default("main")
        .describe("branch id ('main' = trunk) or a document id (list via document tab / documents API)"),
      from: z
        .number()
        .int()
        .min(0)
        .optional()
        .describe("first event sequence to include — scope the recipe to one decision's span"),
      to: z
        .number()
        .int()
        .min(0)
        .optional()
        .describe("last event sequence to include (inclusive)"),
      include_params: z
        .boolean()
        .default(true)
        .describe("include each step's verbatim recorded params (set false for a compact plan outline)"),
    },
    async ({ reference, from, to, include_params }) => {
      try {
        const qs: string[] = [];
        if (from !== undefined) qs.push(`from=${from}`);
        if (to !== undefined) qs.push(`to=${to}`);
        const r = await api(
          "GET",
          `/api/timeline/recipe/${encodeURIComponent(reference)}` +
            (qs.length ? `?${qs.join("&")}` : ""),
        );
        const steps = (r.steps ?? []).map((s: any) => ({
          sequence: s.sequence,
          op_kind: s.op_kind,
          ...(include_params ? { params: s.params } : {}),
          inputs: s.inputs,
          outputs: s.outputs,
          intent: s.intent ?? null,
          checkpoint: s.checkpoint?.name ?? null,
          ...(s.checkpoint ? {} : { checkpoint_absent_reason: s.checkpoint_absent_reason }),
          reissue: s.reissue ?? null,
          ...(s.reissue ? {} : { reissue_absent_reason: s.reissue_absent_reason }),
        }));
        return ok({
          source: r.source,
          step_count: r.step_count,
          sequence_range: r.sequence_range,
          // Disclosed, not smoothed over: a gapped log is not a whole plan.
          sequence_contiguous: r.sequence_contiguous,
          undecodable_events: r.undecodable_events,
          checkpoints: (r.checkpoints ?? []).map((c: any) => ({
            name: c.name,
            description: c.description,
            covers: c.covers,
            covers_is_empty: c.covers_is_empty,
          })),
          certificate_summary: r.certificate_summary,
          reparameterize: r.reparameterize,
          steps,
        });
      } catch (e) {
        // An unknown reference is a TYPED 404 (`document_not_found`) — an
        // honest answer about which recipes exist, not a transport failure.
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "timeline_branch",
    "Fork a timeline branch before speculative work. The fork is an isolated " +
      "EVENT-LOG lane (authorship, audit, merge approval) — the live kernel " +
      "scene stays shared. Authorship records this agent automatically; pass " +
      "agent_id only to label a different logical agent. Set activate:true to " +
      "also record subsequent ops onto the new branch.",
    {
      name: z.string().min(1).describe("branch name, e.g. 'explore-rib-variants'"),
      parent: z.string().default("main").describe("parent branch id ('main' or a branch UUID)"),
      agent_id: z
        .string()
        .optional()
        .describe("logical agent identity recorded as branch author; omit = this agent"),
      description: z.string().optional().describe("what this branch explores"),
      activate: z
        .boolean()
        .default(false)
        .describe("also switch kernel recording to the new branch"),
    },
    async ({ name, parent, agent_id, description, activate }) => {
      try {
        const branch = await api("POST", "/api/branches", {
          name,
          parent,
          agent_id,
          description,
        });
        let recording_on_branch = false;
        if (activate && branch?.id) {
          await api("POST", "/api/branches/active", { branch_id: branch.id });
          recording_on_branch = true;
        }
        return ok({ branch, recording_on_branch });
      } catch (e) {
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "timeline_branches",
    "List timeline branches: id, name, state, author, agent_id, event counts, " +
      "fork point. Filter by state or agent_id (per-agent grouping is a " +
      "projection over recorded branch authorship).",
    {
      state: z
        .enum(["active", "merged", "abandoned", "completed"])
        .optional()
        .describe("keep only branches in this state"),
      agent_id: z.string().optional().describe("keep only branches authored by this agent id"),
    },
    async ({ state, agent_id }) => {
      try {
        const r = await api("GET", "/api/branches");
        let branches: any[] = Array.isArray(r) ? r : [];
        if (state) branches = branches.filter((b) => b.state === state);
        if (agent_id) branches = branches.filter((b) => b.agent_id === agent_id);
        return ok({ count: branches.length, branches });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "timeline_switch",
    "Switch the kernel's RECORDING branch: subsequent geometry ops append " +
      "their events to this branch. Process-global (one live model, one " +
      "recording head) — switch back to 'main' when the exploration is done.",
    {
      branch: z.string().describe("branch id to record onto ('main' or a branch UUID)"),
    },
    async ({ branch }) => {
      try {
        const r = await api("POST", "/api/branches/active", { branch_id: branch });
        return ok(r);
      } catch (e) {
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "timeline_merge",
    "Merge a branch into a target and get the merge's EVIDENCE, not a bool: " +
      "typed conflict witnesses (subject, taxonomy verdict, both colliding " +
      "events) when it refuses; statistics plus the target's per-feature " +
      "rebuild certificate when it lands. 'fast-forward' on diverged branches " +
      "returns a typed refusal carrying the divergence shape.",
    {
      source: z.string().describe("branch id to merge FROM"),
      target: z.string().default("main").describe("branch id to merge INTO"),
      strategy: z
        .enum(["three-way", "fast-forward", "squash"])
        .default("three-way")
        .describe("'three-way' detects + reports typed conflicts; 'fast-forward' refuses on divergence"),
      message: z.string().optional().describe("squash commit message (squash strategy only)"),
      certify: z
        .boolean()
        .default(true)
        .describe("on success, attach the target branch's rebuild certificate"),
    },
    async ({ source, target, strategy, message, certify }) => {
      try {
        const r = await api(
          "POST",
          `/api/branches/${encodeURIComponent(source)}/merge`,
          { target, strategy, message },
        );
        if (r?.success && certify) {
          // The certificate is the proof the merged state still derives
          // (#64 rebuild certificate) — fetched, never fabricated; on
          // failure the reason is surfaced instead of a fake verdict.
          try {
            r.certificate = await api(
              "GET",
              `/api/timeline/rebuild-certificate/${encodeURIComponent(target)}`,
            );
          } catch (e) {
            r.certificate = null;
            r.certificate_unavailable =
              e instanceof Error ? e.message : String(e);
          }
        }
        return ok(r);
      } catch (e) {
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "timeline_conflicts",
    "Read-only merge preview: how source relates to target (up_to_date / " +
      "fast_forward / divergent with event counts) plus the exact typed " +
      "conflict witnesses a three-way merge would report. Nothing is merged; " +
      "no branch state flips. Decide HOW to resolve before committing.",
    {
      source: z.string().describe("branch id to preview merging FROM"),
      target: z.string().default("main").describe("branch id to preview merging INTO"),
    },
    async ({ source, target }) => {
      try {
        const r = await api(
          "GET",
          `/api/branches/${encodeURIComponent(source)}/conflicts?target=${encodeURIComponent(target)}`,
        );
        return ok(r);
      } catch (e) {
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "timeline_checkpoint",
    "Declare design INTENT, opening the next feature (mutating calls are " +
      "refused until a checkpoint is open). Name it in engineering language " +
      "('bolt circle 8 x D18 on D160 B.C.'; 'step 3'-style names are " +
      "refused); the matching notebook line is written automatically. " +
      "Captures the branch's event range; returns the checkpoint id.",
    {
      name: z
        .string()
        .min(1)
        .describe("the intent: feature + governing dimensions + where it sits"),
      description: z.string().optional().describe("reasoning behind the intent (mirrored to the notebook)"),
      branch: z.string().default("main").describe("branch whose event range to capture"),
      // GATE 6 ESCAPE (gates.ts, verification_scope). Opening a checkpoint
      // CLOSES the previous one; if geometry was built under it and never
      // checked with verify_part / verify_claim, this call is refused typed.
      // This flag is the one way through, and it is deliberately an ARGUMENT
      // rather than an omission: the intent then closes unverified ON THE
      // RECORD. Read by the gate before dispatch, AND forwarded to the
      // backend (2026-08-15, item 4 — S3/S11's "true only in the MCP
      // process's RAM" finding): `POST /api/timeline/checkpoint` now
      // persists it onto the created checkpoint
      // (`timeline_engine::Checkpoint::skip_verification`), so the escape
      // survives a restart and is retrievable via timeline_checkpoints —
      // not merely a permitted call. Only sent as `true` when the caller
      // actually passed it AND gate 6 would actually have refused without it
      // (L1, 2026-08-15 final review, `gate6WouldRefuse` in gates.ts) —
      // gate 6 is TS-only, so nothing on the backend re-checks whether there
      // really was anything to skip the way it does for `acknowledge_unsound`;
      // forwarding the flag unconditionally would let the durable record
      // assert an escape that was never taken.
      skip_verification: z
        .boolean()
        .optional()
        .describe(
          "close the PREVIOUS intent without verifying what it built (scratch " +
            "geometry, a cutter about to be subtracted away). Escapes the " +
            "verification gate explicitly instead of silently",
        ),
    },
    async ({ name, description, branch, skip_verification }) => {
      try {
        // L1: forward the flag only when it would actually be escaping
        // something — read BEFORE the call, while intentUnverified still
        // reflects the state the gate itself just evaluated for this exact
        // dispatch (nothing else runs between the gate and this handler).
        const escapedSomething =
          skip_verification === true && gate6WouldRefuse();
        const r = await api("POST", "/api/timeline/checkpoint", {
          name,
          description,
          branch,
          ...(escapedSomething ? { skip_verification: true } : {}),
        });
        // NOTEBOOK MIRROR (audit 2026-08-01 §5): the policy used to ask for a
        // separate blackboard_add_entry carrying the same intent — a two-call
        // ritual the model could half-do. The handler writes the line itself,
        // so "the notebook and the timeline describe one event" is structural.
        // Best-effort with an honest sidecar: a failed mirror never voids the
        // recorded checkpoint, and is named rather than silently absent.
        let notebook_entry: Record<string, unknown>;
        try {
          const line = await api("POST", "/api/blackboard/entries", {
            text: `**Intent** — ${name}${description ? `. ${description}` : ""}`,
            author: "agent",
          });
          notebook_entry = { id: line?.id ?? null };
        } catch (e) {
          notebook_entry = {
            unavailable: e instanceof Error ? e.message : String(e),
          };
        }
        return ok({ checkpoint: r, notebook_entry });
      } catch (e) {
        return refusalOrFail(e);
      }
    },
  );

  server.tool(
    "timeline_checkpoints",
    "List named design states (checkpoints): id, name, description, captured " +
      "event range, author, timestamp — newest first.",
    {},
    async () => {
      try {
        const r = await api("GET", "/api/timeline/checkpoints");
        const checkpoints = Array.isArray(r) ? r : [];
        return ok({ count: checkpoints.length, checkpoints });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "timeline_undo",
    "Step this agent's timeline cursor back one operation and re-derive the " +
      "live model to match. The cursor is per-MCP-session, seeded at the " +
      "current head on first use; at the beginning it answers " +
      "{success:false, can_undo:false} — an honest bottom, not an error.",
    {},
    async () => {
      try {
        const r = await api("POST", "/api/timeline/undo", {
          session_id: AGENT_SESSION_ID,
        });
        return ok(r);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "timeline_redo",
    "Step this agent's timeline cursor forward one operation (after " +
      "timeline_undo) and re-derive the live model to match. At the head it " +
      "answers {success:false} honestly.",
    {},
    async () => {
      try {
        const r = await api("POST", "/api/timeline/redo", {
          session_id: AGENT_SESSION_ID,
        });
        return ok(r);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "clear_timeline",
    "Reset a branch to ZERO events and wipe the live model — DESTRUCTIVE and " +
      "irreversible (the ledger is rewritten). Use clear_parts instead for an " +
      "empty scene with preserved history.",
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
