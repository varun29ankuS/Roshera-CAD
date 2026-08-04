/**
 * The blackboard is per-document, all the way (Varun, 2026-08-04) — proof
 * that a part-scoped write is now INEXPRESSIBLE at the MCP tool schema, not
 * merely discouraged in prose.
 *
 * Before this change, `blackboard_add_entry` / `blackboard_edit_entry` /
 * `blackboard_list` / `blackboard_clear` all spread a `SCOPE_ARGS` object
 * (`part_id` / `scope`) into their zod shape, so an agent could still write
 * into a part's own notebook — a notebook nothing in the frontend displays
 * any more (it only shows the document notebook). That is a write path with
 * no reader: the exact defect class this pass removes everywhere else.
 *
 * This closes it at the SCHEMA:
 *   1. the tool's advertised JSON schema (what `tools/list` and
 *      `describe_tool` actually show an agent) declares no `part_id`/`scope`
 *      property at all;
 *   2. even if a caller sends `part_id`/`scope` anyway, zod's default
 *      "strip unknown keys" behaviour means the value never reaches the
 *      handler — parsing `{text, part_id}` through the registered schema
 *      yields data with no `part_id` key.
 *
 * Build the fixture first (never touches dist/):
 *   npx tsc -p tsconfig.json --outDir test/.build
 * Run:
 *   node test/blackboard_scope.test.mjs
 */

import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

const { buildTable } = await import(
  pathToFileURL(join(HERE, ".build", "surface.js")).href
);
const { toolJsonSchema } = await import(
  pathToFileURL(join(HERE, ".build", "registry.js")).href
);

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

const table = buildTable();

// The four verbs that used to spread SCOPE_ARGS. `blackboard_list` is a
// READ tool but is included too: post-union there is nothing left to
// select (the document view already contains everything), so it lost its
// scope arguments along with the writers.
const SCOPED_TOOLS = [
  "blackboard_add_entry",
  "blackboard_edit_entry",
  "blackboard_list",
  "blackboard_clear",
];

for (const name of SCOPED_TOOLS) {
  check(`${name} is registered`, () => {
    assert.ok(table.has(name), `${name} must still exist as a tool`);
  });

  check(`${name}'s advertised schema declares no part_id/scope property`, () => {
    const entry = table.get(name);
    assert.ok(entry, `${name} not found in the table`);
    const jsonSchema = toolJsonSchema(entry);
    const props = jsonSchema.properties ?? {};
    assert.ok(
      !("part_id" in props),
      `${name}'s inputSchema still advertises part_id: ${JSON.stringify(jsonSchema)}`,
    );
    assert.ok(
      !("scope" in props),
      `${name}'s inputSchema still advertises scope: ${JSON.stringify(jsonSchema)}`,
    );
  });
}

check(
  "blackboard_add_entry: a part_id sent anyway never reaches the handler (stripped by the schema)",
  () => {
    const entry = table.get("blackboard_add_entry");
    const parsed = entry.schema.safeParse({
      text: "note about finger_L3",
      author: "agent",
      part_id: "8",
      scope: "part:8",
    });
    assert.ok(parsed.success, "a call with extra keys must still validate (zod strips, not rejects)");
    assert.ok(
      !("part_id" in parsed.data),
      `part_id survived validation into the handler's args: ${JSON.stringify(parsed.data)}`,
    );
    assert.ok(
      !("scope" in parsed.data),
      `scope survived validation into the handler's args: ${JSON.stringify(parsed.data)}`,
    );
    assert.equal(parsed.data.text, "note about finger_L3", "the real field still parses");
  },
);

check(
  "blackboard_edit_entry: part_id/scope sent anyway never reach the handler",
  () => {
    const entry = table.get("blackboard_edit_entry");
    const parsed = entry.schema.safeParse({
      id: "bb-1",
      text: "revised",
      part_id: "8",
      scope: "part:8",
    });
    assert.ok(parsed.success);
    assert.ok(!("part_id" in parsed.data) && !("scope" in parsed.data));
  },
);

check("blackboard_list: takes no arguments at all", () => {
  const entry = table.get("blackboard_list");
  const parsed = entry.schema.safeParse({ part_id: "8", scope: "part:8" });
  assert.ok(parsed.success, "extra keys are stripped, not rejected");
  assert.deepEqual(parsed.data, {}, "no field survives — there is nothing left to select");
});

check("blackboard_clear: takes no arguments at all", () => {
  const entry = table.get("blackboard_clear");
  const parsed = entry.schema.safeParse({ part_id: "8", scope: "part:8" });
  assert.ok(parsed.success);
  assert.deepEqual(parsed.data, {});
});

if (failures > 0) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log("\nblackboard_scope: all checks passed — part-scoped writes are inexpressible");
