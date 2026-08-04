/**
 * BLACKBOARD — agent/human shared notebook of editable, event-logged lines.
 * Backend-persisted (GET/POST/PATCH/DELETE /api/blackboard*); a line added
 * here shows up live in the frontend Blackboard panel.
 *
 * ## One notebook per document (2026-08-04)
 *
 * The blackboard used to be addressable per PART (`part_id`/`scope`
 * arguments on every verb here) so a 100-part assembly wouldn't mix every
 * part's calculations into one notebook. Varun reversed that: the agent
 * session is already scoped per document, and the notebook the human reads
 * now matches it 1:1 — there is exactly one notebook per document, always.
 *
 * These tools therefore no longer accept a scope of any kind — there is
 * nothing to select, so the argument is gone from the schema entirely
 * rather than merely unused (an agent that still tried to target a part
 * would be writing into a notebook nothing displays, the exact
 * write-with-no-reader defect this closes). `blackboard_list` / the
 * document read still surfaces lines written under the old per-part model
 * before this change (the backend unions them in, tagged with which part
 * they were about — see `api-server/src/blackboard.rs`'s
 * `BlackboardManager::document_snapshot`), so nothing already written
 * became unreadable; there is simply no way to WRITE a new part-scoped line
 * from here any more.
 */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ok, fail } from "../core.js";

/** Wire shape of one Blackboard line (mirrors the frontend store). */
interface BlackboardLine {
  id: string;
  text: string;
  author: "user" | "agent";
  createdAt: number;
  updatedAt: number;
  /** Present only on a line unioned in from a legacy part-scoped notebook
   *  (see the module doc above) — which part it was originally about. */
  partId?: number;
}

// ── ask_choice: a closed question as a renderable card, BY CONSTRUCTION ────
//
// The board renders a ```roshera:choices``` fence as clickable buttons
// (roshera-app `ChoicesCard`); prose enumerations only get buttons through a
// deliberately narrow detector ("Option A:" at line start, ≥2 of them) that
// must never invent options — so a numbered list or inline prose produces no
// card and the human retypes the answer. Rather than loosening that
// detector, this tool makes the fenced form the path of least resistance:
// hand it the question and options, get back a fence that is correct by
// construction. The scalars are emitted with `JSON.stringify`, which is
// valid YAML (double-quoted flow scalars, newlines escaped as `\n`), so no
// authored text — colons, quotes, dashes, even line breaks — can change the
// YAML's shape or terminate the markdown fence early.

/** One option for `ask_choice`. `label` defaults to `value` when omitted. */
export interface AskChoiceOption {
  value: string;
  label?: string;
  detail?: string;
}

/**
 * Why (question, options) cannot become a valid choices card, or `null`
 * when it can. Typed refusals, never silent repairs: a padded, duplicated,
 * or blank option set is refused with the exact defect named — the
 * `.goosehints` contract ("every option must be one you would actually
 * accept") starts with the set being well-formed.
 */
export function askChoiceRefusal(
  question: string,
  options: AskChoiceOption[],
): string | null {
  if (question.trim().length === 0) {
    return "question is empty — a choices card must ask something";
  }
  if (options.length < 2) {
    return `a closed question needs at least 2 options, got ${options.length} — for a single proposal ask in prose instead`;
  }
  const seen = new Set<string>();
  for (const [i, o] of options.entries()) {
    if (o.value.trim().length === 0) {
      return `options[${i}].value is empty — the value is sent verbatim as the human's reply, it cannot be blank`;
    }
    if ((o.label ?? o.value).trim().length === 0) {
      return `options[${i}].label is empty — button text cannot be blank`;
    }
    if (seen.has(o.value)) {
      return `options share the value '${o.value}' — the clicked value must identify ONE option`;
    }
    seen.add(o.value);
  }
  return null;
}

/**
 * The exact ```roshera:choices``` fence the Blackboard renders as buttons.
 * Callers must have passed `askChoiceRefusal` first; this function only
 * formats. Shape mirrors `choicesCardSchema` (roshera-app
 * `blackboard-cards.ts`): `question` + `options[{value,label,detail?}]`,
 * YAML body, `selected` never authored here (the UI adds it when the human
 * answers).
 */
export function buildChoicesFence(question: string, options: AskChoiceOption[]): string {
  const lines = ["```roshera:choices", `question: ${JSON.stringify(question)}`, "options:"];
  for (const o of options) {
    lines.push(`  - value: ${JSON.stringify(o.value)}`);
    lines.push(`    label: ${JSON.stringify(o.label ?? o.value)}`);
    if (o.detail !== undefined && o.detail.trim().length > 0) {
      lines.push(`    detail: ${JSON.stringify(o.detail)}`);
    }
  }
  lines.push("```");
  return lines.join("\n");
}

export function registerBlackboardTools(server: ToolHost) {
  server.tool(
    "blackboard_add_entry",
    "Your notebook TO the human: show your working — given values, derivation, " +
      "result, design rationale (markdown + $math$; the human sees each line " +
      "live and can edit it). Write it UNPROMPTED whenever a dimension, " +
      "tolerance, or shape came from a calculation or a decision worth " +
      "defending. Always the one document-wide notebook. Returns the line id.",
    {
      text: z.string().describe("markdown + $math$ source for the line"),
      author: z.enum(["agent", "user"]).default("agent").describe("who the line is attributed to"),
    },
    async ({ text, author }) => {
      try {
        const line = (await api("POST", "/api/blackboard/entries", {
          text,
          author,
        })) as BlackboardLine;
        return ok({ id: line.id, author: line.author, text: line.text });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "blackboard_edit_entry",
    "EDIT a Blackboard line by id (from blackboard_list); appears live. Keep the " +
      "notebook truthful: when a recalculation changes a number you already " +
      "wrote, update the line rather than appending a correction.",
    {
      id: z.string().describe("line id from blackboard_list"),
      text: z.string().describe("new markdown + $math$ source"),
    },
    async ({ id, text }) => {
      try {
        const line = (await api(
          "PATCH",
          `/api/blackboard/entries/${encodeURIComponent(id)}`,
          { text },
        )) as BlackboardLine;
        return ok({ id: line.id, author: line.author, text: line.text });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "blackboard_list",
    "READ the Blackboard notebook: lines (id, author, text) in order. The human " +
      "can add and edit lines too — read it to pick up their notes and replies. " +
      "The one document-wide notebook; a line written under the old per-part " +
      "model before 2026-08-04 carries `partId` naming which part it was about.",
    {},
    async () => {
      try {
        const snap = (await api("GET", "/api/blackboard")) as {
          lines?: BlackboardLine[];
        };
        const lines = (snap.lines ?? []).map((l) => ({
          id: l.id,
          author: l.author,
          text: l.text,
          ...(l.partId !== undefined ? { partId: l.partId } : {}),
        }));
        return ok({ count: lines.length, lines });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "ask_choice",
    "THE way to ask the human a closed question (a clearance class, a " +
      "standard, a process, a datum): pass the question and the options, get " +
      "back a ```roshera:choices``` fence the Blackboard renders as clickable " +
      "buttons — correctly formed by construction, so the human never retypes " +
      "your list. Post the returned fence VERBATIM as its own line with " +
      "blackboard_add_entry (or end your reply with it). Only for a genuinely " +
      "closed set you can name; keep asking in prose when the answer is a " +
      "number, a name, or anything open. Every option must be one you would " +
      "actually accept.",
    {
      question: z.string().describe("the closed question, one sentence"),
      options: z
        .array(
          z.object({
            value: z
              .string()
              .describe("sent verbatim as the human's reply when clicked"),
            label: z.string().optional().describe("button text; defaults to value"),
            detail: z.string().optional().describe("secondary text beneath the label"),
          }),
        )
        .min(2)
        .describe("the closed set, >= 2 options you would all accept"),
    },
    async ({ question, options }) => {
      const reason = askChoiceRefusal(question, options);
      if (reason !== null) {
        return ok({ refused: true, reason });
      }
      return ok({
        fence: buildChoicesFence(question, options),
        next: "post the fence verbatim as its own blackboard line (blackboard_add_entry), or end your reply with it",
      });
    },
  );

  server.tool(
    "blackboard_clear",
    "CLEAR the Blackboard notebook (every line + its event log), including " +
      "any legacy per-part notes it currently unions in. Destructive; no " +
      "geometry change.",
    {},
    async () => {
      try {
        return ok(await api("POST", "/api/blackboard/clear", {}));
      } catch (e) {
        return fail(e);
      }
    },
  );
}
