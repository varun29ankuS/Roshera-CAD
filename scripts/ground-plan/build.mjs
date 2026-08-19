#!/usr/bin/env node
/**
 * Ground plan — BUILD.
 *
 * Runs the extractor and splices its payload into the viewer template, giving
 * one self-contained HTML file with no external assets and no network calls.
 *
 *   node scripts/ground-plan/build.mjs [out.html]
 *
 * Default output is scripts/ground-plan/ground-plan.html, which is gitignored:
 * the page is generated, so the generator is the thing under version control.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { extract } from "./extract-plan.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const TOKEN = "/*__PLAN_JSON__*/";

const out = resolve(process.argv[2] || join(HERE, "ground-plan.html"));
const template = readFileSync(join(HERE, "viewer.html"), "utf8");

if (!template.includes(TOKEN)) {
  console.error(`viewer.html no longer contains ${TOKEN} — nothing to splice into.`);
  process.exit(1);
}

const plan = extract(ROOT, m => process.stderr.write(m + "\n"));
const payload = JSON.stringify(plan);

// The payload sits inside a <script type="application/json"> block, so a
// literal closing tag anywhere in it would end that block early and leave the
// page silently truncated. No current path can contain one, but a future one
// could, and a corrupted plan is worse than a failed build.
if (payload.includes("</script")) {
  console.error("payload contains a closing script tag and would break the page.");
  process.exit(1);
}

writeFileSync(out, template.replace(TOKEN, payload), "utf8");

const t = plan.meta.totals;
console.log(`${out}`);
console.log(`${t.loc.toLocaleString("en-US")} lines · ${t.files} files · ` +
  `${t.structures} structures · ${plan.edges.length} edges · at ${plan.meta.commit}`);
