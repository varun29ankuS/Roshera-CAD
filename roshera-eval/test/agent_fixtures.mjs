/**
 * Shared fixtures for the dry validation of the part-family AGENT scenarios
 * (oracle-20/21/22). No backend, no LLM: transcripts are built by hand, the
 * honest one must pass, every single-mutation lie must fail.
 */
import { Checks } from "../lib/harness.mjs";

export const soundFinal = (over = {}) => ({ sound: true, watertight: true, manifold: true, open_edges: 0, euler: -2, ...over });
export const cyl = (d, axis = [0, 0, 1], origin = [0, 0, 0]) => ({ surface_kind: "cylinder", radius: d / 2, axis, origin });

export function score(oracle, transcript) {
  const t = new Checks("dry");
  oracle(t, transcript);
  return t;
}

/** Run honest cases (must pass) and lies (must fail). Returns failure count. */
export function runDry(name, oracle, honestCases, lies) {
  let failures = 0;
  console.log(`DRY VALIDATION — ${name}`);
  for (const [label, tr] of honestCases) {
    const t = score(oracle, tr);
    if (t.passed) console.log(`  ✓ honest: ${label} passes (${t.items.length} checks)`);
    else {
      failures += 1;
      console.error(`  ✗ honest: ${label} FAILED:`);
      for (const i of t.items.filter((x) => !x.passed)) console.error(`      - ${i.name}: ${i.detail}`);
    }
  }
  for (const { name: lie, base, mutate } of lies) {
    const tr = structuredClone(base);
    mutate(tr);
    const t = score(oracle, tr);
    if (!t.passed) {
      const caught = t.items.filter((x) => !x.passed).map((x) => x.name);
      console.log(`  ✓ lie caught: ${lie}  [${caught[0]}]`);
    } else {
      failures += 1;
      console.error(`  ✗ lie NOT caught: ${lie}`);
    }
  }
  console.log(failures === 0 ? "PASS" : `FAIL — ${failures}`);
  return failures;
}
