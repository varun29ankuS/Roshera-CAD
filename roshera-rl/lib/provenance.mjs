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

  return { sha, dirty: build.dirty === true, reported_by: "server" };
}

/**
 * Stable digest over canonical JSON — object KEY order must not change the
 * value; array ELEMENT order always does, because array order is meaningful
 * data (a claim list, a tool allowlist) and sorting it would silently treat
 * two different sequences as the same one.
 *
 * KNOWN LIMITS, stated rather than silently overclaimed — both fall out of
 * `JSON.stringify`, which this digest is built on, and neither is reachable
 * from data this codebase actually digests (tasks pass through `defineTask`'s
 * strict validation before they ever reach here — mandatory finite numbers,
 * non-empty strings, a closed measure enum — so neither shape below is ever
 * produced by this package's own data):
 *   - a plain-object key whose value is `undefined` is DROPPED, not digested
 *     — `{a: undefined, b: 1}` and `{b: 1}` digest identically;
 *   - a `Date` (or any class instance whose own enumerable keys are empty)
 *     canonicalises to `{}` — `Object.keys(date)` is empty regardless of the
 *     instant it holds, so every `Date` digests the same as every other and
 *     the same as a bare `{}`.
 * Extending `canon` to special-case these is a real option; it was not taken
 * here because doing so would CHANGE this digest's output for values that
 * happen to touch either shape, and this digest's whole purpose — four
 * downstream tasks compare digests for equality — makes a silent output
 * change exactly the kind of gap this module exists to refuse. Values of
 * either shape reaching this function would be a bug in the caller, not a
 * gap to paper over here.
 */
export function digestOf(value) {
  const canon = (v) =>
    Array.isArray(v)
      ? v.map(canon)
      : v && typeof v === "object"
        ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, canon(v[k])]))
        : v;
  return "sha256:" + createHash("sha256").update(JSON.stringify(canon(value))).digest("hex");
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
 * Assemble the block. `attributable` is false whenever ANY identity is absent —
 * it is the single field a consumer filters on, and a corpus that cannot say
 * which rows it can trust is not usable at any size.
 */
export async function buildProvenance({ kernel, policy, task, mcpEntry, harnessRoot }) {
  const mcp = { version: "0.1.0", ...(await fileDigest(mcpEntry)) };
  const harness = mergeHarness(await shaOf(harnessRoot), await dirtyOf(harnessRoot));
  const block = {
    kernel,
    mcp,
    policy: policy.describe(),
    harness,
    task: { id: task.id, family: task.family, digest: digestOf(task) },
  };
  const absent = (o) => o && typeof o === "object" && typeof o.absent === "string";
  block.attributable =
    !absent(kernel) && !absent(mcp) && !absent(harness) && !absent(block.policy);
  return block;
}
