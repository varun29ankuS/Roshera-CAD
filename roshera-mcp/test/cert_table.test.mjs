/**
 * THE FULL CERTIFICATE READS LINE BY LINE.
 *
 * Varun, 2026-11-26: certificates shown "line by line with a right tick at
 * the end with icons ... so that the whole thing looks very professional".
 * `verify_part` leads its answer with `certificateTable` (src/cert_table.ts):
 * one check per line -- icon, full name, measured evidence, verdict at the
 * right edge -- under a header naming the part and the overall verdict.
 * A check that did not run says "not run", never a tick; a check with
 * nothing to judge says "n/a".
 *
 *   Build first:  npx tsc -p tsconfig.json --outDir test/.build
 *   Run:          node test/cert_table.test.mjs
 */

import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const { certificateTable } = await import(pathToFileURL(join(HERE, ".build", "cert_table.js")).href);

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log(`  ok - ${name}`);
  } catch (e) {
    failures += 1;
    console.log(`  not ok - ${name}\n    ${e.message}`);
  }
}

const SOUND = {
  part_id: 7,
  sound: true,
  brep_valid: true,
  brep_watertight: true,
  manifold: true,
  shells_outward: true,
  self_intersection_free: true,
  construction_consistent: true,
  tessellation_clean: true,
  mesh_quality_clean: true,
  eyes_consistent: "not_applicable",
  face_count: 412,
  open_edges: 0,
  nonmanifold_edges: 0,
  euler_characteristic: -2,
};

const lines = (o) => certificateTable(o).split("\n");
const rowOf = (o, name) => {
  const row = lines(o).find((l) => l.includes(name));
  assert.ok(row, `a line names "${name}":\n${certificateTable(o)}`);
  return row;
};

check("the header names the part and the overall verdict", () => {
  assert.match(lines(SOUND)[0], /part 7/);
  assert.match(lines(SOUND)[0], /SOUND$/);
  assert.match(lines({ ...SOUND, sound: false, watertight: false })[0], /UNSOUND$/);
  assert.match(lines({ ...SOUND, sound: undefined })[0], /NO VERDICT$/);
});

check("every check is on its own line, its verdict at the end", () => {
  for (const name of [
    "B-Rep valid",
    "Watertight",
    "Manifold",
    "Shells face outward",
    "No self-intersection",
    "Construction consistent",
    "Tessellation clean",
    "Mesh quality",
    "Dual-view consistent",
  ]) {
    const row = rowOf(SOUND, name);
    assert.match(row, /(✓|✗|n\/a|not run)$/, row);
  }
});

check("the verdicts line up in one column", () => {
  const rows = lines(SOUND).filter((l) => /(✓|✗|n\/a|not run)$/.test(l));
  const ends = new Set(rows.map((l) => [...l].length));
  assert.ok(ends.size <= 3, `ragged right edge: ${[...ends]}`);
  const ticks = rows.filter((l) => l.endsWith("✓")).map((l) => [...l].length);
  assert.equal(new Set(ticks).size, 1, "ticks share one column");
});

check("measured evidence is printed beside its check", () => {
  assert.match(rowOf(SOUND, "B-Rep valid"), /412 faces/);
  assert.match(rowOf(SOUND, "Watertight"), /0 open edges/);
  assert.match(rowOf(SOUND, "Manifold"), /0 non-manifold/);
});

check("a failed check is a cross, a check not run is never a tick", () => {
  const o = { ...SOUND, sound: false, brep_watertight: false, open_edges: 12, self_intersection_free: null };
  assert.match(rowOf(o, "Watertight"), /12 open edges.*✗$/);
  assert.match(rowOf(o, "No self-intersection"), /not run$/);
  assert.doesNotMatch(rowOf(o, "No self-intersection"), /✓/);
});

check("a check with nothing to judge is n/a", () => {
  assert.match(rowOf(SOUND, "Dual-view consistent"), /n\/a$/);
  assert.match(rowOf({ ...SOUND, eyes_consistent: "inconsistent" }, "Dual-view consistent"), /✗$/);
  assert.match(rowOf({ ...SOUND, eyes_consistent: "consistent" }, "Dual-view consistent"), /✓$/);
});

check("every check line starts with its icon", () => {
  for (const l of lines(SOUND).slice(2)) {
    if (/(✓|✗|n\/a|not run)$/.test(l)) assert.match(l, /^\S\s{2}/u, l);
  }
});

// A production call site exists (the disconnection-gate rule).
{
  const { readFileSync } = await import("node:fs");
  const src = readFileSync(join(HERE, "..", "src", "tools", "perception.ts"), "utf8");
  const at = src.indexOf('"verify_part"');
  const next = src.indexOf("server.tool(", at);
  const body = src.slice(at, next < 0 ? undefined : next);
  check("verify_part's first content item is certificateTable(...)", () => {
    const first = body.indexOf("content: [");
    assert.ok(first > 0, "verify_part returns content");
    const table = body.indexOf("certificateTable(", first);
    const json = body.indexOf("JSON.stringify(", first);
    assert.ok(table > 0 && (json < 0 || table < json), "the table comes before the JSON");
  });
}

if (failures) {
  console.log(`cert_table: ${failures} check(s) FAILED`);
  process.exit(1);
}
console.log("cert_table: all checks passed");
