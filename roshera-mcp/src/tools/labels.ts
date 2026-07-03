/**
 * LABELLER — human-readable NAMEs pinned to a topological entity (vertex/edge/
 * face) or a cross-section plane, so agent and user share one vocabulary.
 * Resolution REFUSES on unknown/ambiguous rather than guessing.
 */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail, BASE } from "../core.js";

export function registerLabelTools(server: McpServer) {
  server.tool(
    "label_create",
    "PIN a name to a feature (e.g. call the min-radius face 'throat'). Target " +
      "by id (`entity_id` + `kind`), by DESCRIPTION (`selector` shaped like " +
      "select_face/select_edge — resolves or REFUSES), or as a section " +
      "(`kind`:'section' + `origin` + `normal`). Re-using a name REPLACES it " +
      "(`replaced:true`). Returns the resolved entity/plane.",
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
          "attach by description: a JSON string shaped like the select_face/" +
            "select_edge body, e.g. '{\"kind\":\"cylindrical\",\"extremal\":\"smallest_area\"}'",
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
    "LIST every label on a part: name, kind, world anchor, display color, the " +
      "kernel-MEASURED key dimension (e.g. Ø2.00 mm), the GD&T verdict " +
      "(in_spec/out_of_spec/not_verified), staleness, description.",
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
    "RESOLVE a label name to the live entity id or section plane it pins. " +
      "REFUSES with not_found / dangling — never returns a wrong entity. Turns " +
      "'fillet the throat edge' into a concrete id.",
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
    "AUTO-PROPOSE labels: the kernel recognizes features (throat = min-radius " +
      "station, exit = axis-extremal cap, chamber = max-radius barrel, fillet " +
      "= constant-radius blend) and SUGGESTS name + pinning assertion — it " +
      "does NOT apply them. Confirm one via label_create with the returned " +
      "`selector`. Returns { proposals: [...] }.",
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
    "REMOVE a label by name. deleted:true when it existed; 404 when not — " +
      "reported honestly.",
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
    "RENAME a label, preserving its binding. 404 when the old name is " +
      "unknown; 409 when the new name is taken by a DIFFERENT label (never " +
      "silently clobbers).",
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
}
