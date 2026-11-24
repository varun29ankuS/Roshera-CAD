// The agent policy (roshera-agent-policy.md, written into the in-app agent's
// workspace as AGENTS.md) tells the agent to call tools DIRECTLY by name with
// invoke({name, args}) -- skipping find_tool. Every such call it shows must
// name a registered tool and only arguments that tool's schema has, with every
// required argument present: a policy that drifts from the tools would teach
// the agent calls that fail.
//
// Run: npm run test:gates:build && node test/agent_policy_tools.test.mjs

import { readFileSync } from "node:fs";
import { buildTableWithControls } from "./.build/surface.js";
import { toolJsonSchema } from "./.build/registry.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);

const policy = readFileSync(new URL("../../roshera-agent-policy.md", import.meta.url), "utf8");
const { table } = buildTableWithControls();

const calls = [...policy.matchAll(/invoke\(\{name: '([a-z_]+)', args: \{([^}]*)\}\}\)/g)];
console.log(`(p1) the policy's direct invoke calls (${calls.length})`);
if (calls.length >= 5) pass(`${calls.length} direct calls shown`);
else fail(`only ${calls.length} direct calls found -- the pattern or the policy changed`);

for (const [, name, rawArgs] of calls) {
  const entry = table.get(name);
  if (!entry) {
    fail(`'${name}' is not a registered tool`);
    continue;
  }
  const schema = toolJsonSchema(entry);
  const props = Object.keys(schema.properties ?? {});
  const required = schema.required ?? [];
  // Argument names: "part_id" or "point: [...]" -> the identifier before ':' or ','.
  const shown = rawArgs
    .replace(/\[[^\]]*\]/g, "")
    .split(",")
    .map((s) => s.split(":")[0].trim())
    .filter(Boolean);
  const unknown = shown.filter((a) => !props.includes(a));
  const missing = required.filter((r) => !shown.includes(r));
  if (unknown.length === 0 && missing.length === 0) pass(`${name}(${shown.join(", ")}) matches its schema`);
  else fail(`${name}: unknown args [${unknown.join(", ")}], missing required [${missing.join(", ")}]`);
}

if (failures > 0) {
  console.error(`\nFAIL -- ${failures} problem(s)`);
  process.exit(1);
}
console.log("\nPASS -- the policy teaches calls that work");
