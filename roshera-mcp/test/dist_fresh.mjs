/**
 * dist freshness — the code goose and roshera-rl RUN is the code the tests TEST.
 *
 * Every host spawns `dist/index.js`; every gate test compiles `src/` into
 * `test/.build`. Nothing tied the two together, so on 2026-11-04 production
 * was running a 2026-09-06 dist six commits behind src (e.g. the
 * `recording_failures` disclosure: 19 occurrences in src, 0 in dist) while
 * every gate test passed. `dist/` is gitignored, so only a check like this
 * can notice.
 *
 * Same tsconfig, same compiler → byte-identical output. Run after
 * `test:gates:build`:  npm run test:dist-fresh
 */
import { readdir, readFile } from "node:fs/promises";
import { join, dirname, relative } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const BUILD = join(HERE, ".build");
const DIST = join(HERE, "..", "dist");

async function jsFiles(root) {
  const out = [];
  async function walk(dir) {
    let entries;
    try {
      entries = await readdir(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = join(dir, e.name);
      if (e.isDirectory()) await walk(p);
      else if (e.name.endsWith(".js")) out.push(relative(root, p));
    }
  }
  await walk(root);
  return out.sort();
}

const built = await jsFiles(BUILD);
if (built.length === 0) {
  console.error("dist_fresh: test/.build is empty — run `npm run test:gates:build` first");
  process.exit(2);
}

const problems = [];
for (const f of built) {
  const fresh = await readFile(join(BUILD, f));
  let shipped;
  try {
    shipped = await readFile(join(DIST, f));
  } catch {
    problems.push(`missing from dist: ${f}`);
    continue;
  }
  if (!fresh.equals(shipped)) problems.push(`stale in dist: ${f}`);
}

if (problems.length > 0) {
  console.error(
    `dist_fresh: dist/ does not match src/ (${problems.length} file(s)) — ` +
      `hosts run dist, so they are running code the tests never saw.\n  ` +
      problems.join("\n  ") +
      `\nFix: npm run build (then reconnect the MCP client).`,
  );
  process.exit(1);
}
console.log(`dist_fresh: dist/ matches src/ (${built.length} files)`);
