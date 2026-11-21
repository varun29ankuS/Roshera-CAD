// The drawing agent mode's parts and prompt (spec
// docs/research/2026-11-20-agent-eyes-on-drawings-design.md, slice 4).
//
// The parts come from the drawing bench (roshera-eval/drawing-bench): the
// CADGenBench reconstructions split dev / held-out by split.json (fixed by
// hash before any tuning), and the NIST MBE models. The choice is a fixed
// rule stated here before any agent result was seen: the FIRST n of each list
// in split.json's order (NIST: file-name order) among the parts the bench
// DRAWS -- the checked-in floors.json says which (a part with no sheet has
// nothing for the agent to improve; nist_ctc_01 fails at import and
// nist_ctc_02 at certify as of 11-21, and the bench reports those). Never pick
// parts by how the agent did on them, and never tune the prompt or the
// playbook on held-out or NIST results.

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const BENCH = resolve(here, "..", "..", "roshera-eval", "drawing-bench");
const STEPS =
  process.env.ROSHERA_BENCH_STEPS ??
  "C:\\Users\\Varun Sharma\\Downloads\\standard-manufacturing-co_smc-harness-v1-2-fable-5-2-max_20261113-034752";
const NIST = process.env.ROSHERA_BENCH_NIST ?? join(BENCH, "reference-nist");

export const SELECTION_RULE =
  "first n of split.json dev and held_out in file order, first n NIST STEP files by name, each among the parts floors.json records as drawn; fixed before any agent result";

/** -> [{ id, split: "dev"|"held_out"|"nist", step }] */
export function drawingParts({ dev = 5, heldOut = 3, nist = 2 } = {}) {
  const split = JSON.parse(readFileSync(join(BENCH, "split.json"), "utf8"));
  const floors = JSON.parse(readFileSync(join(BENCH, "floors.json"), "utf8")).parts;
  const drawn = (id) => floors[id]?.drawn === true;
  const cadgen = (ids, n, name) =>
    ids
      .map((id) => ({ id, split: name, step: join(STEPS, id, "output.step") }))
      .filter((p) => existsSync(p.step) && drawn(p.id))
      .slice(0, n);
  const nistDir = join(NIST, "step");
  const nistParts = existsSync(nistDir)
    ? readdirSync(nistDir)
        .filter((f) => /\.(stp|step)$/i.test(f))
        .sort()
        .map((f) => {
          // nist_ctc_01_asme1_rd.stp -> nist_ctc_01 (bench.py's id)
          const bits = f.split("_");
          const id = bits.length > 2 ? `nist_${bits[1]}_${bits[2]}` : f.replace(/\.[^.]+$/, "");
          return { id, split: "nist", step: join(nistDir, f) };
        })
        .filter((p) => drawn(p.id))
        .slice(0, nist)
    : [];
  return [...cadgen(split.dev, dev, "dev"), ...cadgen(split.held_out, heldOut, "held_out"), ...nistParts];
}

/** The task as the agent reads it: the sheet exists, the loop is the job. */
export function drawingPrompt({ drawingId, partId }) {
  return [
    `An engineering drawing has already been made from an imported STEP part: drawing ${drawingId}, of part ${partId}.`,
    "Your job is to make that sheet professional by improving its layout. Do NOT change the part, and do not make a new drawing.",
    'First read the drafting playbook: kb_lookup with kind "playbook" and key "manufacturing_drawing". Then run its IMPROVE LOOP on drawing ' +
      `${drawingId} exactly as it says, until it tells you to stop.`,
    'Reach the drawing tools (drawing_spaces, drawing_look and the layout verbs) with find_tool and invoke, or by switching the workbench to the "drawing" bench.',
    "Every number you report must come from a tool result. Finish with the report the playbook's loop asks for.",
  ].join("\n");
}
