/**
 * One-off generator for the two ingest_rows fixtures. Run manually
 * (`node test/fixtures/build_fixtures.mjs` from roshera-rl) whenever the
 * fixtures need regenerating from the real trajectory; not part of any test
 * run or the `test` script.
 *
 * `complete.jsonl` starts from a REAL saved trajectory —
 * .superpowers/sdd/2026-08-13-rl-episode-loop-slice1/live-durability-verified/
 * cylinder-r25-h60-0-0.jsonl — which carries genuine STEP and RECIPE shapes
 * from a live run against a real kernel (create_cylinder + verify_part,
 * a recipe with two re-issuable steps, one carrying a reissue mapping and
 * one stating why it has none). Two things are added because the format
 * moved after that file was written:
 *   - a `provenance` block on the header (current shape per provenance.mjs /
 *     provenance.test.mjs's "a complete block is attributable" fixture)
 *   - one REFUSAL step. Zero refusals occur in ANY saved real trajectory
 *     (grepped), so this step is necessarily synthetic — but its shape is
 *     not invented: `refusal:{gate,reason}` matches episode.mjs:382-387
 *     exactly, the gate name `unsound_base` and its reason template are
 *     copied verbatim from roshera-mcp/src/gates.ts:602-612
 *     (unsoundBaseGateRefusal), and `reward.components.refused` /
 *     `reward_final.components.refusals` are bumped the way reward.mjs
 *     actually counts them (mergeFinal, reward.mjs:230).
 *
 * `malformed.jsonl` is a truncated write: a valid header line followed by a
 * line that is not valid JSON — the shape a killed process leaves behind,
 * not an invented schema violation.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, dirname } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const realPath = join(
  here, "..", "..", "..",
  ".superpowers", "sdd", "2026-08-13-rl-episode-loop-slice1",
  "live-durability-verified", "cylinder-r25-h60-0-0.jsonl",
);
const real = readFileSync(realPath, "utf8").trim().split("\n").map((l) => JSON.parse(l));
const realHeader = real.find((l) => l.kind === "header");
const realSteps = real.filter((l) => l.kind === "step");
const realTerminal = real.find((l) => l.kind === "terminal");

const header = {
  ...realHeader,
  tool_allowlist: [...realHeader.tool_allowlist, "boolean_subtract"],
  provenance: {
    kernel: { sha: "3a9375f9", dirty: false, reported_by: "server" },
    mcp: {
      version: "0.1.0",
      dist_digest: "sha256:1f3a9c4e7b2d5f60a8c1e3b7d9f2a4c6e8b0d2f4a6c8e0b2d4f6a8c0e2b4d6f8",
    },
    policy: {
      kind: "scripted",
      script_digest: "sha256:7d2b4f6a8c0e2b4d6f8a0c2e4b6d8f0a2c4e6b8d0f2a4c6e8b0d2f4a6c8e0b2d",
    },
    harness: { sha: "ceb18f2d", dirty: false },
    task: {
      id: "cylinder-r25-h60", family: "cylinder",
      digest: "sha256:3c5e7a9b1d3f5a7c9e1b3d5f7a9c1e3b5d7f9a1c3e5b7d9f1a3c5e7b9d1f3a5c",
    },
    attributable: true,
  },
};

const refusalStep = {
  kind: "step",
  i: 3,
  action: { tool: "boolean_subtract", args: { base: 0, tool: 0 } },
  result_digest: "fnv1a64:aaaaaaaaaaaaaaaa",
  reward: {
    components: { refused: "unsound_base" },
    gaps: [{
      name: "sound",
      reason: "the call was refused by gate \"unsound_base\", so soundness was never measured — there is no verdict to report",
    }],
  },
  refusal: {
    gate: "unsound_base",
    reason: "part 0 is UNSOUND by the kernel's live verdict (unsound: self-intersecting shell) — " +
      "'boolean_subtract' would stack new work onto a defective solid, and every downstream " +
      "certificate would inherit the defect.",
  },
  ms: 88,
};

const terminal = {
  ...realTerminal,
  reward_final: {
    ...realTerminal.reward_final,
    components: { ...realTerminal.reward_final.components, refusals: 1 },
  },
};

const lines = [header, ...realSteps, refusalStep, terminal].map((o) => JSON.stringify(o));
writeFileSync(join(here, "complete.jsonl"), lines.join("\n") + "\n");

const malformedLines = [
  JSON.stringify(realHeader),
  '{"kind":"step","i":0,"action":{"tool":"create_cylinder","args":{"radius":25,"heig',
];
writeFileSync(join(here, "malformed.jsonl"), malformedLines.join("\n") + "\n");

process.stdout.write("wrote complete.jsonl and malformed.jsonl\n");
