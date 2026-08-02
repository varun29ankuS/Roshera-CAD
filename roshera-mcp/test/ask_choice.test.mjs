/**
 * ask_choice proof — the closed-question card is correct BY CONSTRUCTION.
 *
 * The Blackboard renders a ```roshera:choices``` fence as clickable buttons;
 * prose enumerations only get buttons through a deliberately narrow detector
 * that must never invent options. ask_choice makes the fenced form the path
 * of least resistance, so this test proves the construction end to end:
 *
 *   1. the emitted fence round-trips through the SAME YAML parser the
 *      frontend card path uses (`yaml`, resolved from roshera-app's own
 *      node_modules — a different implementation than the emitter, not the
 *      emitter re-run), yielding exactly the (question, options) handed in;
 *   2. the payload satisfies the frontend `choicesCardSchema` constraints
 *      (non-empty question, >= 1 option, non-empty value + label, `selected`
 *      never authored);
 *   3. hostile authored text (colons, quotes, dashes, newlines, backtick
 *      runs) cannot change the YAML's shape or terminate the markdown fence
 *      early (no payload line may start with ```);
 *   4. malformed sets are refused with the defect named — typed refusals,
 *      never silent repairs.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/ask_choice.test.mjs
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

const { buildChoicesFence, askChoiceRefusal } = await import(
  pathToFileURL(join(HERE, ".build", "tools", "blackboard.js")).href
);
const { buildTable, exposedNamesFor } = await import(
  pathToFileURL(join(HERE, ".build", "surface.js")).href
);

// The frontend's own YAML parser (roshera-app/src/lib/blackboard-cards.ts
// does `import { parse } from 'yaml'`) — the independent oracle. Resolved
// from roshera-app's node_modules so emitter and parser share zero code.
const requireApp = createRequire(
  join(HERE, "..", "..", "roshera-app", "package.json"),
);
const { parse: parseYaml } = requireApp("yaml");

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log(`  PASS  ${name}`);
  } catch (e) {
    failures++;
    console.error(`  FAIL  ${name}\n        ${e.message}`);
  }
}

/** Strip the fence markers; assert the frame is exactly as the renderer
 *  expects (` ```roshera:choices ` opener, ` ``` ` closer, both alone on
 *  their line). Returns the YAML body. */
function bodyOf(fence) {
  const lines = fence.split("\n");
  assert.equal(lines[0], "```roshera:choices", "opener line");
  assert.equal(lines[lines.length - 1], "```", "closer line");
  const body = lines.slice(1, -1);
  for (const l of body) {
    assert.ok(
      !l.startsWith("```"),
      `payload line must not start a fence: ${JSON.stringify(l)}`,
    );
  }
  return body.join("\n");
}

/** The frontend `choicesCardSchema` constraints, asserted structurally. */
function assertCardShape(card) {
  assert.equal(typeof card.question, "string");
  assert.ok(card.question.length >= 1, "question non-empty");
  assert.ok(Array.isArray(card.options) && card.options.length >= 1);
  for (const o of card.options) {
    assert.equal(typeof o.value, "string");
    assert.ok(o.value.length >= 1, "value non-empty");
    assert.equal(typeof o.label, "string");
    assert.ok(o.label.length >= 1, "label non-empty");
    if (o.detail !== undefined) assert.equal(typeof o.detail, "string");
  }
  assert.equal(card.selected, undefined, "selected is UI-authored, never ours");
}

// ─── 1+2. The .goosehints example, round-tripped through the real parser ───

check("clearance-class example round-trips through the frontend's parser", () => {
  const question = "Which clearance class for the M8 holes?";
  const options = [
    {
      value: "close",
      label: "Close (H12) - 9.0 mm",
      detail: "tighter location, less assembly slop",
    },
    {
      value: "medium",
      label: "Medium (H13) - 10.0 mm",
      detail: "the usual default for bolted joints",
    },
  ];
  assert.equal(askChoiceRefusal(question, options), null);
  const fence = buildChoicesFence(question, options);
  const card = parseYaml(bodyOf(fence));
  assertCardShape(card);
  assert.deepEqual(card, { question, options });
});

check("label defaults to value when omitted", () => {
  const fence = buildChoicesFence("Datum for the bore axis?", [
    { value: "A" },
    { value: "B", detail: "the mounting face" },
  ]);
  const card = parseYaml(bodyOf(fence));
  assertCardShape(card);
  assert.equal(card.options[0].label, "A");
  assert.equal(card.options[1].label, "B");
  assert.equal(card.options[0].detail, undefined);
  assert.equal(card.options[1].detail, "the mounting face");
});

// ─── 3. Hostile authored text cannot change the shape ──────────────────────

check("colons, quotes, dashes, newlines, backticks survive verbatim", () => {
  const question = 'Standard: use "ISO 2768-mK: general"?\nOr per-feature?';
  const options = [
    { value: "yes: adopt it", label: '- "yes" (house default)' },
    {
      value: "no\nper-feature",
      label: "no — tolerance each feature",
      detail: "``` is not special here",
    },
  ];
  assert.equal(askChoiceRefusal(question, options), null);
  const fence = buildChoicesFence(question, options);
  const card = parseYaml(bodyOf(fence));
  assertCardShape(card);
  assert.deepEqual(card, { question, options });
});

// ─── 4. Typed refusals — the defect named, nothing repaired ────────────────

check("empty question is refused", () => {
  const r = askChoiceRefusal("   ", [{ value: "a" }, { value: "b" }]);
  assert.ok(r !== null && r.includes("question is empty"), r ?? "no refusal");
});

check("a single option is refused (ask in prose instead)", () => {
  const r = askChoiceRefusal("Proceed?", [{ value: "yes" }]);
  assert.ok(r !== null && r.includes("at least 2"), r ?? "no refusal");
});

check("duplicate values are refused", () => {
  const r = askChoiceRefusal("Which?", [
    { value: "close", label: "Close (H12)" },
    { value: "close", label: "Medium (H13)" },
  ]);
  assert.ok(r !== null && r.includes("share the value"), r ?? "no refusal");
});

check("blank value is refused", () => {
  const r = askChoiceRefusal("Which?", [{ value: "  " }, { value: "b" }]);
  assert.ok(r !== null && r.includes("value is empty"), r ?? "no refusal");
});

check("blank label is refused", () => {
  const r = askChoiceRefusal("Which?", [
    { value: "a", label: " " },
    { value: "b" },
  ]);
  assert.ok(r !== null && r.includes("label is empty"), r ?? "no refusal");
});

// ─── Surface: full table + labels bench, not minimal-resident ──────────────

check("ask_choice is registered (full surface), not minimal-resident", () => {
  const table = buildTable();
  assert.ok(table.has("ask_choice"), "registered in the table");
  assert.ok(
    exposedNamesFor(table, "full").includes("ask_choice"),
    "exposed in full mode",
  );
  assert.ok(
    !exposedNamesFor(table, "minimal").includes("ask_choice"),
    "not in the minimal surface (discovery rides find_tool)",
  );
});

// ─── Card-kind parity: one fact, three statements ──────────────────────────
//
// The renderable card kinds are stated in three places written by different
// hands: the frontend's CARD_KINDS (the CONSTRAINT — anything else renders
// as raw text), the `.goosehints` "Kinds:" paragraph (steering — what the
// agent is told it may emit), and this package's buildChoicesFence (the one
// MCP-side emitter, hard-coding `roshera:choices`). They agree today; these
// checks fail the moment any copy drifts, instead of the drift surfacing as
// an agent card silently rendering as raw text.

const appCardsSrc = readFileSync(
  join(HERE, "..", "..", "roshera-app", "src", "lib", "blackboard-cards.ts"),
  "utf8",
);
const goosehints = readFileSync(join(HERE, "..", "..", ".goosehints"), "utf8");

function frontendCardTruth() {
  const prefixMatch = /const CARD_FENCE_PREFIX\s*=\s*'([^']+)'/.exec(appCardsSrc);
  const kindsMatch = /const CARD_KINDS[^=]*=\s*\[([^\]]*)\]/.exec(appCardsSrc);
  assert.ok(prefixMatch, "CARD_FENCE_PREFIX not found in blackboard-cards.ts");
  assert.ok(kindsMatch, "CARD_KINDS not found in blackboard-cards.ts");
  const kinds = [...kindsMatch[1].matchAll(/'([a-z_]+)'/g)].map((m) => m[1]);
  assert.ok(kinds.length > 0, "CARD_KINDS parsed empty");
  return { prefix: prefixMatch[1], kinds };
}

check("buildChoicesFence emits the exact kind the frontend renders", () => {
  const { prefix, kinds } = frontendCardTruth();
  assert.ok(
    kinds.includes("choices"),
    `frontend CARD_KINDS ${JSON.stringify(kinds)} no longer includes 'choices' — ` +
      "ask_choice's fence would render as raw text",
  );
  const fence = buildChoicesFence("Which?", [{ value: "a" }, { value: "b" }]);
  assert.equal(
    fence.split("\n")[0],
    "```" + prefix + "choices",
    "fence opener must be exactly the frontend's prefix + kind",
  );
});

check(".goosehints 'Kinds:' paragraph names exactly the frontend's kinds", () => {
  const { kinds } = frontendCardTruth();
  const para = /Kinds:([\s\S]*?)\r?\n\r?\n/.exec(goosehints);
  assert.ok(para, "'Kinds:' paragraph not found in .goosehints");
  const stated = [...new Set([...para[1].matchAll(/`([a-z_]+)`/g)].map((m) => m[1]))];
  assert.deepEqual(
    stated.sort(),
    [...kinds].sort(),
    ".goosehints tells the agent a different kind set than the frontend renders — " +
      "update the 'Kinds:' paragraph and CARD_KINDS together",
  );
});

if (failures > 0) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log("\nask_choice: all checks passed");
