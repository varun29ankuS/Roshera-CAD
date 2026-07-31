/**
 * kb_lookup — the tiered engineering knowledge base as ONE MCP tool (vault
 * policy KB, Research/2026-07-31-policy-knowledge-base.md §1.1).
 *
 * The load-bearing surface decision: this tool is registered in the FULL table
 * ONLY — never added to CORE_SURFACE / MINIMAL_SURFACE (surface.ts). Its
 * marginal cost on the resident minimal surface is therefore ZERO; discovery
 * rides find_tool's existing ranking, dispatch rides invoke — the funnel the
 * surface already pays for. test/kb_lookup.mjs pins the minimal bill to its
 * pre-kb_lookup measurement so this placement cannot silently regress.
 *
 * Three kinds, one delivery mechanism:
 *   pack      (Tier 1) — one process's DFM pack: the doc's RETRIEVAL CHUNK +
 *              kernel_certified_rules + the exact dfm_check pack arg (or null
 *              when the kernel cannot check the process at all — §3.0 honesty
 *              table). Uncertified guidance is MARKED uncertified in the
 *              payload; an agent can never present shop practice as a kernel
 *              verdict (the GD&T certified-vs-design-intent split, applied to
 *              manufacturing knowledge).
 *   playbook  (Tier 2) — one feature type's build playbook + tool_sequence of
 *              real MCP tool names in order.
 *   reference (Tier 3) — cited data lookups (kb_reference.ts). Every answer is
 *              {value, source}; [V]/conflicting items refuse by name.
 *
 * No fetch, no backend dependency: the knowledge base is compiled data. All
 * responses echo kind/key and carry token_estimate (ceil(len/4), the same
 * proxy registry.ts/estimateTokens and the kernel registry use).
 */

import { z } from "zod";
import type { ToolHost } from "../registry.js";
import { ok } from "../core.js";
import { PACK_CHUNKS, PLAYBOOK_CHUNKS } from "./kb_data.js";
import { referenceLookup, REFERENCE_KEYS } from "./kb_reference.js";

/** ceil(chars/4) — the house token proxy, applied to a chunk or payload. */
const tokensOf = (s: string): number => Math.ceil(s.length / 4);

// ─── Tier-1 pack metadata (doc §3.0 honesty table, verified against the
//     kernel: RulePackId in dfm/report.rs, dfm_check's enum in inspect.ts) ───

type KernelPresence = "certified" | "schema_slot_no_rules" | "none";

interface PackMeta {
  dfm_check_pack_arg: string | null;
  kernel_certified_rules: string[];
  kernel_presence: KernelPresence;
  section: string;
}

const CERT_TEXT: Record<KernelPresence, string> = {
  certified:
    "kernel-certified ONLY for the rules in kernel_certified_rules (dfm_check verdicts); every other number in text is vendor/published practice [P] — state it, never claim it was checked",
  schema_slot_no_rules:
    "NOT kernel-certified: a RulePackId schema slot exists but ZERO rules are implemented and dfm_check cannot run this pack — all guidance is vendor rule-of-thumb [P], never a kernel verdict",
  none:
    "NOT kernel-certified: no kernel presence at all (no RulePackId, no dfm_check enum value) — all guidance is vendor rule-of-thumb [P], never a kernel verdict",
};

const PACK_META: Record<string, PackMeta> = {
  fdm: {
    dfm_check_pack_arg: "fdm",
    kernel_certified_rules: ["fdm.overhang", "fdm.min_wall", "fdm.min_bore", "fdm.trapped_volume"],
    kernel_presence: "certified",
    section: "§3.1",
  },
  injection_molding: {
    dfm_check_pack_arg: "injection_molding",
    kernel_certified_rules: ["mold.draft"],
    kernel_presence: "certified",
    section: "§3.2",
  },
  sla: { dfm_check_pack_arg: null, kernel_certified_rules: [], kernel_presence: "none", section: "§3.3" },
  sls: { dfm_check_pack_arg: null, kernel_certified_rules: [], kernel_presence: "none", section: "§3.4" },
  cnc_3_axis: { dfm_check_pack_arg: null, kernel_certified_rules: [], kernel_presence: "schema_slot_no_rules", section: "§3.5" },
  cnc_5_axis: { dfm_check_pack_arg: null, kernel_certified_rules: [], kernel_presence: "none", section: "§3.6" },
  sheet_metal: { dfm_check_pack_arg: null, kernel_certified_rules: [], kernel_presence: "schema_slot_no_rules", section: "§3.7" },
  casting: { dfm_check_pack_arg: null, kernel_certified_rules: [], kernel_presence: "none", section: "§3.8" },
};

// ─── Tier-2 playbook metadata: the canonical tool path, in order (doc §4).
//     test/kb_lookup.mjs asserts every name resolves in the live table. ──────

interface PlaybookMeta {
  tool_sequence: string[];
  section: string;
  certification: string;
}

const PLAYBOOK_CERT_DEFAULT =
  "design guidance; only the named tool verdicts (psketch_certify, dfm_check, gdt_fcf, verify_part) are kernel-certified — sizing rules and process couplings quoted in text are practice [P], not verdicts";

const PLAYBOOK_META: Record<string, PlaybookMeta> = {
  hole: {
    tool_sequence: [
      "kb_lookup", "psketch_begin", "psketch_add_entity", "psketch_constrain",
      "psketch_certify", "psketch_extrude", "boolean", "section_view",
      "dfm_check", "gdt_fcf", "verify_part",
    ],
    section: "§4.1",
    certification: PLAYBOOK_CERT_DEFAULT,
  },
  boss: {
    tool_sequence: [
      "kb_lookup", "psketch_begin", "psketch_add_entity", "psketch_constrain",
      "psketch_certify", "psketch_extrude", "boolean", "dfm_check",
      "section_view", "mass_properties", "verify_part",
    ],
    section: "§4.2",
    certification: PLAYBOOK_CERT_DEFAULT,
  },
  rib: {
    tool_sequence: [
      "plane_from_face", "psketch_begin", "psketch_add_entity", "psketch_constrain",
      "psketch_certify", "psketch_extrude", "boolean", "dfm_check",
      "measure_faces", "verify_part",
    ],
    section: "§4.3",
    certification: PLAYBOOK_CERT_DEFAULT,
  },
  gusset: {
    tool_sequence: [
      "psketch_begin", "psketch_add_entity", "psketch_constrain", "psketch_certify",
      "psketch_extrude", "boolean", "dfm_check", "measure_faces", "verify_part",
    ],
    section: "§4.4",
    certification: PLAYBOOK_CERT_DEFAULT,
  },
  bearing_seat: {
    tool_sequence: [
      "kb_lookup", "label_create", "psketch_begin", "psketch_add_entity",
      "psketch_constrain", "psketch_certify", "psketch_extrude", "boolean",
      "gdt_datum", "gdt_fcf", "measure_faces", "verify_part",
    ],
    section: "§4.5",
    certification: PLAYBOOK_CERT_DEFAULT,
  },
  flange: {
    tool_sequence: [
      "psketch_begin", "psketch_add_entity", "psketch_constrain", "psketch_certify",
      "psketch_extrude", "drill_pattern", "fillet_edges", "gdt_datum",
      "gdt_fcf", "dfm_check", "verify_part",
    ],
    section: "§4.6",
    certification: PLAYBOOK_CERT_DEFAULT,
  },
  bolt_pattern: {
    tool_sequence: [
      "kb_lookup", "drill_pattern", "label_create", "gdt_datum", "gdt_fcf",
      "dfm_check", "verify_part",
    ],
    section: "§4.7",
    certification: PLAYBOOK_CERT_DEFAULT,
  },
  snap_fit: {
    tool_sequence: [
      "psketch_begin", "psketch_add_entity", "psketch_constrain", "psketch_certify",
      "psketch_extrude", "boolean", "dfm_check", "measure_faces",
      "mass_properties", "verify_part",
    ],
    section: "§4.8",
    certification:
      "NO kernel/analyzer backing for snap-fit function anywhere in dfm/ — retention force is not kernel-verifiable today; only the generic tool verdicts named in text (dfm_check(fdm) overhang/min_wall, verify_part) are certified",
  },
};

const KB_DOC = "policy KB (vault Research/2026-07-31-policy-knowledge-base.md)";

export function registerKbTools(server: ToolHost): void {
  server.tool(
    "kb_lookup",
    "Tiered engineering KNOWLEDGE BASE — retrieve, don't hold (policy KB " +
      "2026-07-31). kind 'pack' = one manufacturing process's DFM pack (fdm, " +
      "sla, sls, injection_molding, cnc_3_axis, cnc_5_axis, sheet_metal, " +
      "casting): guidance chunk + kernel_certified_rules + the exact dfm_check " +
      "pack arg, or null with the guidance MARKED uncertified when the kernel " +
      "cannot check that process. kind 'playbook' = one feature type's build " +
      "playbook (hole, boss, rib, gusset, bearing_seat, flange, bolt_pattern, " +
      "snap_fit) with tool_sequence of real MCP tools in order. kind " +
      "'reference' = cited engineering data: clearance_hole (bolt clearance " +
      "diameter, ISO 273), tap_drill (tap drill size for a thread), " +
      "general_tolerance (ISO 2768 band), fit_class (ISO 286 hole/shaft fit " +
      "e.g. H7/g6 for a bearing_seat), thread_spec (pitch + tap drill + " +
      "clearance in one record), standard_stock, bend_allowance (sheet-metal " +
      "K-factor), drill_size (nearest standard drill). Every answer carries " +
      "{value, source} — NEVER a bare number; open house questions and " +
      "conflicting vendor data REFUSE by name instead of defaulting.",
    {
      kind: z
        .enum(["pack", "playbook", "reference"])
        .describe("pack = Tier-1 process pack; playbook = Tier-2 feature playbook; reference = Tier-3 cited data lookup"),
      key: z
        .string()
        .min(1)
        .describe("pack: process name (e.g. 'fdm'); playbook: feature type (e.g. 'hole'); reference: data function (e.g. 'clearance_hole')"),
      args: z
        .record(z.any())
        .optional()
        .describe("reference kind only: the data function's parameters, e.g. {fastener:'M6', class:'close'} or {nominal_mm:20, fit:'H7/g6'}"),
    },
    async ({ kind, key, args }) => {
      const k = key.toLowerCase().trim();

      if (kind === "pack") {
        const text = PACK_CHUNKS[k];
        const meta = PACK_META[k];
        if (text === undefined || meta === undefined) {
          return ok({
            kind, key: k, refused: true,
            reason: `unknown process pack '${k}' — the KB covers exactly the 8 processes listed; anything else is a sourcing question, not a guess`,
            valid_keys: Object.keys(PACK_CHUNKS),
          });
        }
        return ok({
          kind, key: k,
          text,
          kernel_certified_rules: meta.kernel_certified_rules,
          dfm_check_pack_arg: meta.dfm_check_pack_arg,
          kernel_presence: meta.kernel_presence,
          certified: meta.kernel_presence === "certified",
          certification: CERT_TEXT[meta.kernel_presence],
          source: `${KB_DOC} ${meta.section}; in-chunk provenance tags ([P] vendor/published, kernel rule ids) apply per line`,
          token_estimate: tokensOf(text),
        });
      }

      if (kind === "playbook") {
        const text = PLAYBOOK_CHUNKS[k];
        const meta = PLAYBOOK_META[k];
        if (text === undefined || meta === undefined) {
          return ok({
            kind, key: k, refused: true,
            reason: `unknown feature playbook '${k}' — the KB covers exactly the 8 feature types listed`,
            valid_keys: Object.keys(PLAYBOOK_CHUNKS),
          });
        }
        return ok({
          kind, key: k,
          text,
          tool_sequence: meta.tool_sequence,
          certification: meta.certification,
          source: `${KB_DOC} ${meta.section}`,
          token_estimate: tokensOf(text),
        });
      }

      // kind === "reference"
      const result = referenceLookup(k, (args ?? {}) as Record<string, unknown>);
      const payload: Record<string, unknown> = {
        kind, key: k,
        ...result,
        ...("refused" in result
          ? {}
          : {
              certification:
                "reference data (published standard / vendor table / published formula, per source) — NOT a kernel measurement or verdict",
            }),
      };
      payload.token_estimate = tokensOf(JSON.stringify(payload));
      return ok(payload);
    },
  );
}

export { REFERENCE_KEYS };
