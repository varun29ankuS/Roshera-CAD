/**
 * Provenance: what produced a trajectory.
 *
 * The governing rule is the project's own: absence is stated with a reason,
 * never defaulted. An identity field that falls back to "unknown" asserts
 * something nobody checked, which is the defect this module exists to remove.
 */

/** Raised when the operator claims one build and the server reports another. */
export class KernelIdentityConflict extends Error {}

/**
 * Ask the SERVER what it is.
 *
 * `claimed` (an operator-supplied sha, e.g. ROSHERA_KERNEL_SHA) is used ONLY to
 * detect disagreement. It is never promoted into the returned identity: the
 * field means "the server said so", and an operator claim is not evidence.
 * A server that cannot say yields a stated absence — including when it is
 * unreachable, because "I could not ask" is a fact about the run, not a crash.
 */
export async function resolveKernelIdentity({ baseUrl, authHeader = {}, claimed }) {
  let build;
  try {
    const res = await fetch(`${baseUrl}/health`, {
      headers: { ...authHeader },
      signal: AbortSignal.timeout(10_000),
    });
    if (!res.ok) {
      return {
        reported_by: "server",
        absent: `the server answered /health with ${res.status}, so it did not state its build`,
      };
    }
    build = (await res.json())?.build;
  } catch (e) {
    return {
      reported_by: "server",
      absent: `the server could not be reached to state its build: ${e?.message ?? e}`,
    };
  }

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
  if (claim !== "" && claim !== sha) {
    throw new KernelIdentityConflict(
      `build identity conflict: the operator claimed ${claim} but the server reports ${sha}. ` +
        `A batch that cannot say which kernel produced it is not producing training data. ` +
        `Unset ROSHERA_KERNEL_SHA to record the server's own answer.`,
    );
  }

  return { sha, dirty: build.dirty === true, reported_by: "server" };
}
