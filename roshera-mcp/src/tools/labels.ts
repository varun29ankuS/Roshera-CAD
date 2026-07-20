/**
 * LABELLER — human-readable NAMEs pinned to a topological entity (vertex/edge/
 * face) or a cross-section plane, so agent and user share one vocabulary.
 * Resolution REFUSES on unknown/ambiguous rather than guessing.
 */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail, BASE, AUTH_HEADERS } from "../core.js";

export function registerLabelTools(server: McpServer) {
  server.tool(
    "label_create",
    "PIN a name to a feature (e.g. 'throat'). Target by id (`entity_id`+`kind`), " +
      "by DESCRIPTION (`selector`, select_face/select_edge shape), or as a " +
      "section (kind:'section' + origin + normal). Re-using a name REPLACES it.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      name: z.string().min(1).describe("the label, e.g. 'throat' (unique per part)"),
      kind: z
        .enum(["vertex", "edge", "face", "section"])
        .describe("entity kind to pin (or 'section' for a cutting plane)"),
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
        .array(z.number()).length(3)
        .optional()
        .describe("section only: a point on the cutting plane"),
      normal: z
        .array(z.number()).length(3)
        .optional()
        .describe("section only: the plane normal"),
      description: z.string().optional().describe("optional free-text note stored with the label"),
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
          headers: { "Content-Type": "application/json", "X-Roshera-Agent": "Claude", ...AUTH_HEADERS },
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
    "LIST a part's labels: name, kind, world anchor, colour, kernel-MEASURED key " +
      "dimension (e.g. Ø2.00 mm), GD&T verdict, staleness, description.",
    { part_id: z.number().int().describe("part id (list_parts)") },
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
      "REFUSES not_found / dangling — never a wrong entity.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      name: z.string().min(1).describe("the label name to resolve"),
    },
    async ({ part_id, name }) => {
      try {
        const res = await fetch(
          `${BASE}/api/agent/parts/${part_id}/labels/${encodeURIComponent(name)}/resolve`,
          { headers: { "X-Roshera-Agent": "Claude", ...AUTH_HEADERS } },
        );
        return ok(await res.json());
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "propose_labels",
    "AUTO-PROPOSE labels: the kernel recognizes features (throat, exit, chamber, " +
      "fillet) and SUGGESTS name + pinning `selector` — does NOT apply them. " +
      "Confirm one via label_create with the returned selector.",
    { part_id: z.number().int().describe("part id (list_parts)") },
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
    "REMOVE a label by name. deleted:true when it existed; 404 when not.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
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
    "RENAME a label, preserving its binding. 404 if old unknown; 409 if new name " +
      "is taken by a different label (never clobbers).",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
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
