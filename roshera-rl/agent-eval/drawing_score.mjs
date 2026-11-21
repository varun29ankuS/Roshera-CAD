// Scoring for the drawing agent mode (drawing.mjs).
//
// Two sources, never mixed up:
//   * the KERNEL -- the sheet's space map (grade = its worst space, the count
//     of spaces below the bar) before and after, and the sheet certificate
//     after, read by the harness over REST. These are the result.
//   * the TRANSCRIPT -- which tools the agent called and which layout verbs
//     were refused. A refused verb changes nothing and records nothing, so the
//     kernel cannot report it; the stream is the only witness, and the reward
//     says so per field.
//
// The reward is a NAMED vector, never a sum: weighting a better grade against
// a refused verb is a training choice with no kernel justification
// (roshera-rl/lib/reward.mjs). A reading the harness could not take is ABSENT
// with a reason in `absent`, never a 0.

/** The layout verbs (roshera-mcp/src/tools/drawing_hands.ts). */
export const LAYOUT_VERBS = Object.freeze(new Set([
  "drawing_move_dimension",
  "drawing_align",
  "drawing_move_view",
  "drawing_reattach_leader",
  "drawing_remove_view",
  "drawing_remove_dimension",
]));

/** The agent's eyes (a look shows the sheet; it changes nothing). */
const LOOKS = new Set(["drawing_look"]);

/** Grades are written to 3+ decimals; this rounds float noise away. */
const round6 = (v) => Math.round(v * 1e6) / 1e6 + 0; // + 0 turns -0 into 0

function textOf(content) {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.map((c) => c?.text ?? JSON.stringify(c)).join("\n");
  return JSON.stringify(content ?? "");
}

/**
 * Every tool call in a Claude Code stream-json transcript, in order, paired
 * with its result. `invoke` is unwrapped to the tool it ran. `ok` is null for
 * a call whose result never arrived (the run was cut off): unknown, never
 * assumed fine. `refusal` is the stable code a refused call carried.
 * -> [{ i, tool, args, ok, refusal }]
 */
export function stepsFromStream(text) {
  const steps = [];
  const byId = new Map();
  for (const line of String(text).split(/\r?\n/)) {
    if (!line.trim()) continue;
    let ev;
    try {
      ev = JSON.parse(line);
    } catch {
      continue;
    }
    const content = ev?.message?.content;
    if (!Array.isArray(content)) continue;
    if (ev.type === "assistant") {
      for (const c of content) {
        if (c?.type !== "tool_use") continue;
        let tool = String(c.name ?? "").replace(/^mcp__roshera__/, "");
        let args = c.input ?? {};
        if (tool === "invoke" && typeof args?.name === "string") {
          tool = args.name;
          args = args.args ?? {};
        }
        const step = { i: steps.length, tool, args, ok: null, refusal: null };
        steps.push(step);
        byId.set(c.id, step);
      }
    } else if (ev.type === "user") {
      for (const c of content) {
        if (c?.type !== "tool_result") continue;
        const step = byId.get(c.tool_use_id);
        if (!step) continue;
        const body = textOf(c.content);
        const failed = c.is_error === true || /"success"\s*:\s*false/.test(body);
        step.ok = !failed;
        if (failed) {
          step.refusal =
            /"refusal"\s*:\s*"([a-z_]+)"/.exec(body)?.[1] ??
            /"error_code"\s*:\s*"([a-z_]+)"/.exec(body)?.[1] ??
            "unstated";
        }
      }
    }
  }
  return steps;
}

/**
 * The episode's reward vector. `before` / `after` are space maps
 * (GET /api/drawings/{id}/spaces), `certificateAfter` the sheet certificate
 * (GET /api/drawings/{id}/certificate); any may be null when the harness
 * could not read it.
 */
export function drawingReward({ before, after, certificateAfter, steps }) {
  const r = { absent: {}, sources: {} };
  const kernel = (key, value, source, missing) => {
    if (value === undefined || value === null) r.absent[key] = missing;
    else {
      r[key] = value;
      r.sources[key] = source;
    }
  };
  const map = "kernel: GET /api/drawings/{id}/spaces";
  const cert = "kernel: GET /api/drawings/{id}/certificate";
  kernel("grade_before", before?.grade, map + " before the agent ran", "no space map before the agent ran");
  kernel("below_bar_before", before?.below_bar, map + " before the agent ran", "no space map before the agent ran");
  kernel("grade_after", after?.grade, map + " after the agent ran", "no space map after the agent ran");
  kernel("below_bar_after", after?.below_bar, map + " after the agent ran", "no space map after the agent ran");
  if (r.grade_before !== undefined && r.grade_after !== undefined) {
    r.grade_delta = round6(r.grade_after - r.grade_before);
    r.sources.grade_delta = "kernel: grade_after - grade_before";
  } else {
    r.absent.grade_delta = "a grade was not read on both sides";
  }
  kernel("sound_after", certificateAfter?.sound, cert, "no certificate after the agent ran");
  kernel("complete_after", certificateAfter?.complete, cert, "no certificate after the agent ran");
  const verbs = (steps ?? []).filter((s) => LAYOUT_VERBS.has(s.tool));
  const transcript = "transcript: the agent's stream (a refused verb records nothing the kernel could count)";
  r.verbs_applied = verbs.filter((s) => s.ok === true).length;
  r.verbs_refused = verbs.filter((s) => s.ok === false).length;
  r.verbs_unanswered = verbs.filter((s) => s.ok === null).length;
  r.looks = (steps ?? []).filter((s) => LOOKS.has(s.tool) && s.ok === true).length;
  r.refusals_by_code = {};
  for (const s of verbs) {
    if (s.ok === false) r.refusals_by_code[s.refusal] = (r.refusals_by_code[s.refusal] ?? 0) + 1;
  }
  for (const k of ["verbs_applied", "verbs_refused", "verbs_unanswered", "looks", "refusals_by_code"]) {
    r.sources[k] = transcript;
  }
  return r;
}

/** The closed roshera-rl outcome for one Claude Code run. */
export function drawingOutcome(exit) {
  if (exit?.timedOut) return "BUDGET_EXHAUSTED";
  if (exit?.code !== 0) return "CRASHED";
  return "COMPLETED";
}
