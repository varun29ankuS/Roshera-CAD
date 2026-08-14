/**
 * Provenance: what produced a trajectory.
 *
 * The governing rule is the project's own: absence is stated with a reason,
 * never defaulted. An identity field that falls back to "unknown" asserts
 * something nobody checked, which is the defect this module exists to remove.
 */

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);

/** Raised when the operator claims one build and the server reports another. */
export class KernelIdentityConflict extends Error {}

/** Raised by `digestOf` for a value it cannot represent without colliding. */
export class UndigestableValue extends Error {}

/**
 * Ask the SERVER what it is.
 *
 * `claimed` (an operator-supplied sha, e.g. ROSHERA_KERNEL_SHA) is used ONLY to
 * detect disagreement. It is never promoted into the returned identity: the
 * field means "the server said so", and an operator claim is not evidence.
 * A server that cannot say yields a stated absence — including when it is
 * unreachable or times out, because "I could not ask" is a fact about the
 * run, not a crash. Each way of not-answering states which one it was:
 * unreachable, timed out, answered with an error status, or answered with a
 * body that was not JSON are four different facts and get four different
 * sentences. `timeoutMs` is exposed only so tests can exercise the timeout
 * path without waiting on the real default.
 */
export async function resolveKernelIdentity({ baseUrl, authHeader = {}, claimed, timeoutMs = 10_000 }) {
  let res;
  try {
    res = await fetch(`${baseUrl}/health`, {
      headers: { ...authHeader },
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (e) {
    if (e?.name === "TimeoutError") {
      return {
        reported_by: "server",
        absent: `the server did not answer /health within ${timeoutMs}ms, so it did not state its build`,
      };
    }
    return {
      reported_by: "server",
      absent: `the server could not be reached to state its build: ${e?.message ?? e}`,
    };
  }

  if (!res.ok) {
    return {
      reported_by: "server",
      absent: `the server answered /health with ${res.status}, so it did not state its build`,
    };
  }

  let body;
  try {
    body = await res.json();
  } catch (e) {
    return {
      reported_by: "server",
      absent: `the server answered /health but its body was not valid JSON, so it did not state its build: ${e?.message ?? e}`,
    };
  }
  const build = body?.build;

  if (!build || typeof build !== "object") {
    return {
      reported_by: "server",
      absent: "the server's /health carried no `build` object — it is older than this contract",
    };
  }

  if (typeof build.sha !== "string" || build.sha.trim() === "") {
    return {
      reported_by: "server",
      absent:
        typeof build.absent === "string" && build.absent.trim() !== ""
          ? build.absent
          : "the server reported a build with no sha and stated no reason",
    };
  }

  const sha = build.sha.trim();
  const claim = typeof claimed === "string" ? claimed.trim() : "";
  // Compare case-insensitively (shas are canonically lowercase, but an
  // operator claim differing only in case still AGREES); the sha recorded
  // below is always the server's own, exact casing.
  if (claim !== "" && claim.toLowerCase() !== sha.toLowerCase()) {
    throw new KernelIdentityConflict(
      `build identity conflict: the operator claimed ${claim} but the server reports ${sha}. ` +
        `A batch that cannot say which kernel produced it is not producing training data. ` +
        `Unset ROSHERA_KERNEL_SHA to record the server's own answer.`,
    );
  }

  // A MISSING `dirty` IS AN ABSENCE, NOT A CLEAN TREE. `build.dirty === true`
  // turned a server that stated a sha and nothing else — an older or newer
  // contract, a proxy that strips fields — into `dirty: false`, a positive
  // claim of cleanliness nobody made. The build IS identified, so this is not
  // a kernel-wide absence: the reason rides on its own key, `dirty_absent`,
  // which `identityDetermined` (below) deliberately does not treat as an
  // `absent` — a sha with no dirty reading is still an attributable identity.
  if (typeof build.dirty !== "boolean") {
    return {
      sha,
      reported_by: "server",
      dirty_absent:
        build.dirty === undefined
          ? "the server stated a sha but no dirty reading, so whether its tree was clean is unknown"
          : `the server's dirty reading was ${JSON.stringify(build.dirty)}, not a boolean, so it states nothing about whether its tree was clean`,
    };
  }
  return { sha, dirty: build.dirty, reported_by: "server" };
}

/**
 * Stable digest over canonical JSON — object KEY order must not change the
 * value; array ELEMENT order always does, because array order is meaningful
 * data (a claim list, a tool allowlist) and sorting it would silently treat
 * two different sequences as the same one.
 *
 * ─── IT REFUSES RATHER THAN COLLIDES ──────────────────────────────────────
 *
 * A digest is an IDENTITY CLAIM. `JSON.stringify`, which this is built on,
 * quietly erases several shapes, and an erased shape is not an approximation
 * — it is two different values asserting the same identity. Every one of
 * these was MEASURED against this module before the guard below existed:
 *
 *   digestOf({a: undefined, b: 1}) === digestOf({b: 1})     // key DROPPED
 *   digestOf({t: new Date(0)})     === digestOf({t: {}})    // Date → {}
 *   digestOf({t: new Date(0)})     === digestOf({t: new Date(99999)})
 *   digestOf({r: NaN})             === digestOf({r: null})  // non-finite → null
 *   digestOf({m: new Map([["a",1]])}) === digestOf({m: {}}) // internal slots
 *
 * So `canon` accepts ONLY what JSON carries losslessly — plain objects
 * (`Object.prototype` or a null prototype), arrays, strings, FINITE numbers,
 * booleans, `null` — and throws `UndigestableValue` for anything else,
 * naming the offending key path (`$.args.when`) so the caller can find it.
 *
 * A SENTINEL ENCODING WAS REJECTED, deliberately. Mapping `undefined` to
 * `{__undefined__: true}` and a `Date` to its ISO string does narrow the
 * collision — but it does not close it: the sentinel is itself expressible,
 * so `{a: undefined}` would then collide with a literal
 * `{a: {__undefined__: true}}`. That trades a known collision for a rarer one
 * under a docstring claiming the gap is closed, which is the exact failure
 * this header used to contain. A refusal cannot be wrong.
 *
 * ─── WHAT A REFUSAL COSTS, MEASURED AT EVERY CALL SITE ────────────────────
 *
 * Seven call sites digest data, and only ONE of them sees data validated by
 * `defineTask` — the claim this header used to make, that "neither shape is
 * reachable from this package's own data", was false:
 *
 *   1. `fileDigest` (below)               — a base64 string. Total.
 *   2. `buildProvenance` (below)          — the `defineTask`-validated task.
 *   3. `policy.mjs:97`  scriptedPolicy    — a CALLER-SUPPLIED script, validated
 *      by nothing. This is where all five collisions above are reachable, and
 *      the value it produces (`script_digest`) IS the policy's identity.
 *   4. `policy.mjs:178` referencePolicy   — a constant string. Total.
 *   5. `ingest/rows.mjs:120`              — `run_id`.
 *   6. `ingest/rows.mjs:138`              — `episode_id`.
 *   7. `ingest/store.mjs:108`             — `rl_policy`'s primary key.
 *
 * Sites 5-7 read data that has already been through `JSON.parse`, which
 * cannot produce `undefined`, a `Date`, a `Map` or a non-finite number, so no
 * refusal is reachable there at all. Sites 3 and 2 are reached only from
 * `policy.describe()` inside `buildProvenance`, which `runner.mjs` already
 * wraps: a throw there is recorded as that ONE episode's `SETUP_FAILED`
 * carrying this message, and its siblings are untouched. A refusal therefore
 * costs one recorded episode, never a batch.
 *
 * ─── LIMITS THAT REMAIN, TRUE ONES ────────────────────────────────────────
 *
 *   - `-0` digests identically to `0` (`JSON.stringify(-0) === "0"`).
 *   - Only OWN ENUMERABLE properties are read, so a non-enumerable own
 *     property is not part of the digest. Own SYMBOL keys are refused rather
 *     than ignored, because unlike the above they can carry arbitrary data.
 */
export function digestOf(value) {
  return "sha256:" + createHash("sha256").update(JSON.stringify(canon(value, "$"))).digest("hex");
}

function refuse(path, what, why) {
  return new UndigestableValue(
    `digestOf cannot represent ${what} at ${path}: ${why}. A digest is an identity ` +
      `claim, so this is refused rather than digested to a value it would share with ` +
      `a different input.`,
  );
}

/** JSON-canonical form, or a throw naming the path of the first value that has none. */
function canon(v, path) {
  if (v === null) return null;
  const t = typeof v;
  if (t === "string" || t === "boolean") return v;
  if (t === "number") {
    if (!Number.isFinite(v)) {
      throw refuse(path, `the non-finite number ${String(v)}`,
        "JSON renders NaN and ±Infinity alike as null, so it would digest identically to a real null and to every other non-finite number");
    }
    return v;
  }
  if (t === "undefined") {
    throw refuse(path, "an `undefined` value",
      "JSON.stringify DROPS the key entirely, so `{a: undefined, b: 1}` would digest identically to `{b: 1}`");
  }
  if (t === "function" || t === "symbol" || t === "bigint") {
    throw refuse(path, `a ${t}`, "JSON cannot carry one");
  }
  if (Array.isArray(v)) {
    const out = [];
    for (let i = 0; i < v.length; i += 1) {
      if (!(i in v)) {
        throw refuse(`${path}[${i}]`, "a sparse-array hole",
          "JSON renders a hole as null, so it would digest identically to an explicit null element");
      }
      out.push(canon(v[i], `${path}[${i}]`));
    }
    return out;
  }
  const proto = Object.getPrototypeOf(v);
  if (proto !== Object.prototype && proto !== null) {
    throw refuse(path, `a ${v?.constructor?.name ?? "class"} instance`,
      "its data lives in internal slots, not own enumerable keys — a Date, Map or Set canonicalises to a bare `{}`, so every instant (or every map) would digest the same");
  }
  if (Object.getOwnPropertySymbols(v).length > 0) {
    throw refuse(path, "an object with own symbol keys",
      "JSON.stringify ignores them, so their data would vanish from the identity without a trace");
  }
  // `Object.create(null)`, NOT `{}`: on an ordinary object literal the
  // assignment `out["__proto__"] = …` invokes `Object.prototype`'s
  // `__proto__` SETTER instead of creating an own property, so the key —
  // which `JSON.parse` does produce as a real own property — vanishes from
  // the digest. Two blocks differing only inside `__proto__` would then
  // share an identity, which is the one thing this function exists to
  // prevent. A null-prototype object has no such setter, so the key lands
  // as data. (Line 230 already accepts a null prototype as input, so this
  // is also the shape `canon` is willing to be handed.)
  const out = Object.create(null);
  for (const k of Object.keys(v).sort()) {
    out[k] = canon(v[k], `${path}.${k}`);
  }
  return out;
}

async function fileDigest(path) {
  try {
    return { dist_digest: digestOf((await readFile(path)).toString("base64")) };
  } catch (e) {
    return { absent: `the MCP entry could not be read to digest it (${path}): ${e?.message ?? e}` };
  }
}

async function dirtyOf(cwd) {
  try {
    const { stdout } = await run("git", ["status", "--porcelain"], { cwd });
    return { dirty: stdout.trim() !== "" };
  } catch (e) {
    return { absent: `git could not report whether the tree was dirty: ${e?.message ?? e}` };
  }
}

async function shaOf(cwd) {
  try {
    const { stdout } = await run("git", ["rev-parse", "--short", "HEAD"], { cwd });
    return { sha: stdout.trim() };
  } catch (e) {
    return { absent: `git could not report the harness commit: ${e?.message ?? e}` };
  }
}

/**
 * Combine the sha and dirty readings into one `harness` object. A plain
 * `{...shaResult, ...dirtyResult}` spread loses `shaResult.absent` whenever
 * BOTH calls fail, because they share the one key `absent` and the second
 * spread silently overwrites the first — a corpus reading the surviving
 * reason would see only "could not report whether the tree was dirty" and
 * never learn the sha call failed too. This block's contract is that an
 * absence carries A reason, and losing one of two real reasons to a spread
 * collision is exactly the kind of silent narrowing that contract exists to
 * forbid. When only one call fails, its `absent` and the other's real field
 * (`sha` or `dirty`) sit on different keys and coexist without collision, so
 * only the both-failed case needs special handling.
 */
function mergeHarness(shaResult, dirtyResult) {
  const shaAbsent = typeof shaResult.absent === "string";
  const dirtyAbsent = typeof dirtyResult.absent === "string";
  if (shaAbsent && dirtyAbsent) {
    return { absent: `${shaResult.absent} Separately: ${dirtyResult.absent}` };
  }
  return { ...shaResult, ...dirtyResult };
}

/**
 * The parts of a provenance block that are BATCH-INVARIANT: `mcpEntry` and
 * `harnessRoot` do not vary per episode, so neither should the work of
 * reading one and asking git about the other. A caller running N episodes
 * from the same batch should call this ONCE and pass the result to every
 * `buildProvenance` call — see its `mcp`/`harness` params below.
 *
 * Beyond the wasted file read and two `git` subprocesses per episode,
 * recomputing this per episode risks two episodes IN THE SAME BATCH
 * disagreeing about their own harness: if the tree changes mid-run (a
 * concurrent `git status` catching a checkout in progress), episode 3 could
 * record a different `dirty` reading than episode 1 did a second earlier —
 * an internally inconsistent batch reporting on its own provenance.
 */
export async function resolveBatchIdentity({ mcpEntry, harnessRoot }) {
  const mcp = { version: "0.1.0", ...(await fileDigest(mcpEntry)) };
  const harness = mergeHarness(await shaOf(harnessRoot), await dirtyOf(harnessRoot));
  return { mcp, harness };
}

/**
 * Assemble the block. `attributable` is false whenever ANY identity is absent —
 * it is the single field a consumer filters on, and a corpus that cannot say
 * which rows it can trust is not usable at any size.
 *
 * `mcp`/`harness` may be pre-resolved (via `resolveBatchIdentity`, above) and
 * passed straight through — the batch-invariant path. `mcpEntry`/`harnessRoot`
 * remain accepted directly for a caller building a single, one-off block (this
 * module's own tests do, and any future caller that never had a batch to
 * amortize over): when `mcp`/`harness` are omitted, this resolves them itself,
 * exactly as before.
 */
export async function buildProvenance({ kernel, policy, task, mcpEntry, harnessRoot, mcp, harness }) {
  const resolvedMcp = mcp ?? { version: "0.1.0", ...(await fileDigest(mcpEntry)) };
  const resolvedHarness = harness ?? mergeHarness(await shaOf(harnessRoot), await dirtyOf(harnessRoot));
  const block = {
    kernel,
    mcp: resolvedMcp,
    policy: policy.describe(),
    harness: resolvedHarness,
    task: { id: task.id, family: task.family, digest: digestOf(task) },
  };
  block.attributable =
    identityDetermined(kernel) && identityDetermined(resolvedMcp) &&
    identityDetermined(resolvedHarness) && identityDetermined(block.policy);
  return block;
}

/**
 * Does this dimension carry an identity that was actually DETERMINED?
 *
 * It asserts the POSITIVE SHAPE rather than the absence of an absence. The
 * predicate this replaced was `!(o && typeof o === "object" && typeof o.absent
 * === "string")` — the negation of "an object carrying an `absent` string" —
 * so every other shape passed as "identity determined", `undefined` and a bare
 * string included. Measured: a policy whose `describe()` returned `undefined`
 * produced a block asserting FULL attributability with no policy identity in
 * it at all, and `store.mjs` then wrote no `rl_policy` row either, leaving a
 * corpus row marked attributable whose policy dimension exists nowhere.
 *
 * `policy` is third-party by construction — `runner.mjs` wraps `describe()` in
 * a try/catch precisely because it is — and slice 2 puts real model adapters
 * behind that same seam. A THROWING `describe()` was already handled; this is
 * what handles a LYING one.
 *
 * Three conditions, each load-bearing:
 *   - a non-array OBJECT: a bare string names nothing this block can carry,
 *     and an array is not an identity descriptor;
 *   - NO `absent` key at all (not merely no absent *string*): an `absent` key
 *     of any type is the dimension saying it does not know;
 *   - AT LEAST ONE own key: `{}` asserts nothing, so it must not assert
 *     attributability. Note this deliberately accepts a kernel carrying only
 *     `{sha, reported_by, dirty_absent}` — a sha with no dirty reading is
 *     still a determined identity, and `dirty_absent` is not `absent`.
 *
 * Exported because `ingest/rows.mjs` must recompute the same verdict from a
 * trajectory file rather than trusting the file's own boolean, and the two
 * must be the SAME predicate — two copies would drift, and the ingester's
 * copy is the one a corpus is filtered on.
 */
export function identityDetermined(o) {
  return o !== null && typeof o === "object" && !Array.isArray(o) &&
    !("absent" in o) && Object.keys(o).length > 0;
}
