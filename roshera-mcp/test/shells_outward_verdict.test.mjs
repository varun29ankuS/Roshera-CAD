/**
 * SHELLS-OUTWARD REACHES THE AGENT.
 *
 * The kernel certificate carries `shells_outward` (every shell faces out of
 * the material: bodies enclose positive signed volume, voids negative) with a
 * `misoriented_shells` witness (api-server/src/main.rs `certificate_json`).
 * The MCP rebuilds the perception with a FIXED key set (core.ts
 * `perceptionFromBody` / `perceive`) and the one-line verdict enumerates a
 * FIXED dimension list (`compactVerdict` DIMS); a conjunct missing from either
 * is dropped before any agent sees it.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/shells_outward_verdict.test.mjs
 */

import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// No backend: the verdict's best-effort unit fetch fails fast and falls back.
process.env.ROSHERA_URL = "http://127.0.0.1:9";

const core = await import(pathToFileURL(join(HERE, ".build", "core.js")).href);

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures += 1;
    console.log(`FAIL ${name}\n     ${e.message}`);
  }
}

const allTrue = {
  brep_valid: true,
  watertight: true,
  manifold: true,
  self_intersection_free: true,
  tessellation_clean: true,
  mesh_quality_clean: true,
};

check("an inside-out shell is named as the failed dimension", () => {
  const line = core.compactVerdict({ ...allTrue, sound: false, shells_outward: false });
  assert.match(line, /UNSOUND/);
  assert.match(line, /failed: shells-outward/);
});

check("a verified outward part lists shells-outward among the verified", () => {
  const line = core.compactVerdict({ ...allTrue, sound: true, shells_outward: true });
  assert.match(line, /SOUND ✓/);
  assert.match(line, /shells-outward/);
  assert.doesNotMatch(line, /unverified/);
});

check("a hot-path verdict that did not compute it reports it unverified", () => {
  const line = core.compactVerdict({ ...allTrue, sound: true, shells_outward: null });
  assert.match(line, /unverified: shells-outward/);
});

if (failures > 0) {
  console.log(`\n${failures} check(s) failed`);
  process.exit(1);
}
console.log("\nall shells_outward verdict checks passed");
