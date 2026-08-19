#!/usr/bin/env node
/**
 * Ground plan — DATA EXTRACTOR.
 *
 * Walks the working tree and emits every figure the plan renders: districts,
 * structures, individual files, and the edges between them. The viewer is a
 * renderer of this output and invents nothing, so re-running this is the only
 * step needed to make the plan current.
 *
 * Everything here is MEASURED. The only authored content in the finished page
 * is the prose annotations, which live in the viewer template keyed by
 * structure, and which carry no numbers.
 *
 *   node scripts/ground-plan/extract-plan.mjs [repo-root] > plan.json
 *
 * Also importable: `import { extract } from "./extract-plan.mjs"`.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join, relative, sep, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

/** Path prefix → structure. Ordered: FIRST MATCH WINS, so a specific module
 *  must precede the crate root that would otherwise swallow it. */
const STRUCTURES = [
  // The kernel — geometry-engine
  { k: "A",  d: "kernel", n: "Booleans & blends",     pre: "roshera-backend/geometry-engine/src/operations" },
  { k: "B",  d: "kernel", n: "B-Rep topology",        pre: "roshera-backend/geometry-engine/src/primitives" },
  { k: "C",  d: "kernel", n: "Exact predicates",      pre: "roshera-backend/geometry-engine/src/math" },
  { k: "D",  d: "kernel", n: "Sketch solver",         pre: "roshera-backend/geometry-engine/src/sketch2d" },
  { k: "E",  d: "kernel", n: "Drawings",              pre: "roshera-backend/geometry-engine/src/drawing" },
  { k: "F",  d: "kernel", n: "Meshing",               pre: "roshera-backend/geometry-engine/src/tessellation" },
  { k: "G",  d: "kernel", n: "Manufacturability",     pre: "roshera-backend/geometry-engine/src/dfm" },
  { k: "H",  d: "kernel", n: "Kernel gates",          pre: "roshera-backend/geometry-engine/src/harness" },
  { k: "I",  d: "kernel", n: "Queries & fidelity",    pre: "roshera-backend/geometry-engine/src/queries" },
  { k: "J",  d: "kernel", n: "Assembly maths",        pre: "roshera-backend/geometry-engine/src/assembly" },
  { k: "K",  d: "kernel", n: "Raster render",         pre: "roshera-backend/geometry-engine/src/render" },
  { k: "L",  d: "kernel", n: "Readable geometry",     pre: "roshera-backend/geometry-engine/src/readable" },
  { k: "M",  d: "kernel", n: "GD&T",                  pre: "roshera-backend/geometry-engine/src/gdt" },
  { k: "N",  d: "kernel", n: "Kernel utilities",      pre: "roshera-backend/geometry-engine/src" },
  // The server
  { k: "P",  d: "server", n: "Route handlers",        pre: "roshera-backend/api-server/src/handlers" },
  { k: "O",  d: "server", n: "HTTP & WebSocket core", pre: "roshera-backend/api-server/src" },
  { k: "Q",  d: "server", n: "Timeline",              pre: "roshera-backend/timeline-engine/src" },
  { k: "R",  d: "server", n: "Sessions & RBAC",       pre: "roshera-backend/session-manager/src" },
  // The agent surface
  { k: "S",  d: "agent",  n: "MCP tool surface",      pre: "roshera-mcp/src" },
  { k: "T",  d: "agent",  n: "Model providers",       pre: "roshera-backend/ai-integration/src" },
  { k: "U",  d: "agent",  n: "RL episode bridge",     pre: "roshera-rl" },
  { k: "V",  d: "agent",  n: "Verdict harness",       pre: "roshera-backend/verdict-harness/src" },
  // What comes out
  { k: "W",  d: "out",    n: "Export formats",        pre: "roshera-backend/export-engine/src" },
  { k: "X",  d: "out",    n: "ROS native format",     pre: "roshera-backend/ros-format/src" },
  // The viewport
  { k: "Y",  d: "view",   n: "Viewport components",   pre: "roshera-app/src/components" },
  { k: "Z",  d: "view",   n: "Client libraries",      pre: "roshera-app/src/lib" },
  { k: "AA", d: "view",   n: "State stores",          pre: "roshera-app/src/stores" },
  { k: "AB", d: "view",   n: "App shell",             pre: "roshera-app/src" },
  // The proving ground
  { k: "AC", d: "proof",  n: "Eval scenarios",        pre: "roshera-eval" },
  { k: "AE", d: "proof",  n: "Assembly engine",       pre: "roshera-backend/assembly-engine/src" },
  // The vocabulary
  { k: "AD", d: "vocab",  n: "Shared types",          pre: "roshera-backend/shared-types/src" },
];

const DISTRICTS = [
  { id: "kernel", n: "The kernel",         sub: "geometry-engine" },
  { id: "server", n: "The server",         sub: "api-server · timeline · sessions" },
  { id: "agent",  n: "The agent surface",  sub: "mcp · providers · rl" },
  { id: "out",    n: "What comes out",     sub: "export · ros-format" },
  { id: "view",   n: "The viewport",       sub: "roshera-app" },
  { id: "proof",  n: "The proving ground", sub: "eval · assembly" },
  { id: "vocab",  n: "The vocabulary",     sub: "shared-types" },
];

/** A referenced module name → the structure that owns it. Rust's module tree
 *  is not the file tree, so `use crate::primitives` names a MODULE and the
 *  edge is drawn at module granularity. */
const MOD_TO_STRUCTURE = {
  operations: "A", primitives: "B", math: "C", sketch2d: "D", drawing: "E",
  tessellation: "F", dfm: "G", harness: "H", queries: "I", assembly: "J",
  render: "K", readable: "L", gdt: "M",
  perception: "N", spatial: "N", labels: "N", export: "N", bin: "N", performance: "N",
  handlers: "P", components: "Y", lib: "Z", stores: "AA",
  shared_types: "AD", geometry_engine: "A", timeline_engine: "Q",
  session_manager: "R", export_engine: "W", ros_format: "X",
  assembly_engine: "AE", ai_integration: "T",
};

/** Packages that reach the server over HTTP/WebSocket rather than by linking
 *  it. Kept separate because the difference is load-bearing: the agent surface
 *  is replaceable precisely because it has no link-time coupling to the kernel. */
const WIRE = [
  { from: "S",  to: "O", pkg: "roshera-mcp/src" },
  { from: "Y",  to: "O", pkg: "roshera-app/src" },
  { from: "AC", to: "O", pkg: "roshera-eval" },
  { from: "U",  to: "O", pkg: "roshera-rl" },
];

const SOURCE_EXT = new Set([".rs", ".ts", ".tsx", ".mjs", ".js"]);
const SKIP_DIR = new Set(["node_modules", "target", "dist", ".git", "runs", "build", ".vite"]);
const MIN_EDGE_WEIGHT = 3;
const CALLS_SERVER = /fetch\(|axios|new WebSocket|\/api\//;

function walk(dir, out = []) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const e of entries) {
    if (SKIP_DIR.has(e.name)) continue;
    const full = join(dir, e.name);
    if (e.isDirectory()) walk(full, out);
    else if (SOURCE_EXT.has(e.name.slice(e.name.lastIndexOf(".")))) out.push(full);
  }
  return out;
}

/** Commits in all history that touched each path, in one pass over the log. */
function readChurn(root) {
  const churn = new Map();
  const log = execSync("git log --pretty=tformat:@ --name-only", {
    cwd: root, maxBuffer: 1 << 30, encoding: "utf8",
  });
  for (const line of log.split("\n")) {
    if (!line || line === "@") continue;
    churn.set(line, (churn.get(line) || 0) + 1);
  }
  return churn;
}

/** Module names this file names in its use/import statements. */
function referencesOf(path, src) {
  const refs = new Set();
  if (path.endsWith(".rs")) {
    for (const m of src.matchAll(/use\s+crate::([a-z_0-9]+)/g)) refs.add(m[1]);
    for (const m of src.matchAll(
      /use\s+(shared_types|geometry_engine|timeline_engine|session_manager|export_engine|ros_format|assembly_engine|ai_integration)::/g,
    )) refs.add(m[1]);
  } else {
    for (const m of src.matchAll(/from\s+['"]([^'"]+)['"]/g)) {
      const target = m[1];
      if (!target.startsWith(".") && !target.startsWith("@/") && !target.startsWith("~/")) continue;
      const seg = target.replace(/^[.@~/]+/, "").split("/")[0];
      if (seg) refs.add(seg);
    }
  }
  return [...refs].sort();
}

export function extract(root, log = () => {}) {
  const rel = f => relative(root, f).split(sep).join("/");
  const structureOf = r => STRUCTURES.find(s => r.startsWith(s.pre + "/") || r === s.pre)?.k ?? null;

  log("reading git history…");
  const churn = readChurn(root);

  log("walking tree…");
  const files = [];
  for (const abs of walk(root)) {
    const path = rel(abs);
    const k = structureOf(path);
    if (!k) continue;
    let src;
    try {
      src = readFileSync(abs, "utf8");
    } catch {
      continue;
    }
    files.push({
      p: path,
      k,
      loc: src.split("\n").length,
      c: churn.get(path) || 0,
      t: (src.match(/#\[test\]|#\[tokio::test\]|\bit\(|\btest\(/g) || []).length,
      r: referencesOf(path, src),
      wire: CALLS_SERVER.test(src),
    });
  }
  files.sort((a, b) => b.loc - a.loc);

  const structures = STRUCTURES.map(s => {
    const own = files.filter(f => f.k === s.k);
    return {
      k: s.k, d: s.d, n: s.n, path: s.pre,
      loc: own.reduce((a, f) => a + f.loc, 0),
      files: own.length,
      churn: own.reduce((a, f) => a + f.c, 0),
      tests: own.reduce((a, f) => a + f.t, 0),
    };
  }).filter(s => s.files > 0);

  // Reference edges: how many files on one side name the other.
  const weights = new Map();
  for (const f of files) {
    for (const ref of f.r) {
      const to = MOD_TO_STRUCTURE[ref];
      if (!to || to === f.k) continue;
      const id = `${f.k}>${to}`;
      weights.set(id, (weights.get(id) || 0) + 1);
    }
  }
  const edges = [...weights]
    .map(([id, w]) => {
      const [from, to] = id.split(">");
      return { from, to, w, kind: "use" };
    })
    .filter(e => e.w >= MIN_EDGE_WEIGHT)
    .sort((a, b) => b.w - a.w);

  for (const w of WIRE) {
    const n = files.filter(f => f.p.startsWith(w.pkg) && f.wire).length;
    if (n > 0) edges.push({ from: w.from, to: w.to, w: n, kind: "wire" });
  }
  for (const f of files) delete f.wire;

  return {
    meta: {
      project: "Roshera",
      commit: execSync("git rev-parse --short HEAD", { cwd: root, encoding: "utf8" }).trim(),
      generated: new Date().toISOString().slice(0, 10),
      generator: "scripts/ground-plan/extract-plan.mjs",
      totals: {
        loc: files.reduce((a, f) => a + f.loc, 0),
        files: files.length,
        structures: structures.length,
        districts: DISTRICTS.length,
      },
      caveats: [
        "churn = commits touching a path; renames are not followed, so a moved file reads younger than it is",
        "Rust reference edges are module-level: Rust's module tree is not the file tree, so a file-to-file edge would be a guess",
        "a use of the geometry-engine CRATE from outside it is attributed to A, the kernel's largest structure — crate-level granularity, not module-level",
        "loc counts every line, including blanks and comments",
        `reference edges below ${MIN_EDGE_WEIGHT} referencing files are omitted, to keep the plan legible`,
      ],
    },
    districts: DISTRICTS,
    structures,
    edges,
    files,
  };
}

// CLI: write the payload to stdout, progress to stderr.
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = process.argv[2] || join(dirname(fileURLToPath(import.meta.url)), "..", "..");
  const plan = extract(root, m => process.stderr.write(m + "\n"));
  process.stdout.write(JSON.stringify(plan));
  const t = plan.meta.totals;
  process.stderr.write(`${t.files} files · ${t.loc} lines · ${plan.edges.length} edges\n`);
}
