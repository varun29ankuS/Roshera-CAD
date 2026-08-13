/**
 * Reward extraction — a NAMED VECTOR, never a scalar.
 *
 * Weighting soundness against fidelity against refusal count is a training
 * choice with no kernel justification. A single number would assert a
 * tradeoff Roshera cannot prove, so consumers scalarize and this module
 * reports.
 *
 * Refusals are RECORDED, not penalized. A refusal is information: the agent
 * met a constraint and learned where it sits. Whether that is negative reward
 * is the trainer's decision.
 *
 * A component that could not be measured is ABSENT with a stated reason,
 * never 0 — identical discipline to `FidelityReport::gaps`.
 *
 * INPUT: the envelope from `mcp_session.readToolResult`, never a raw tool
 * result and never a hand-shaped object. That is deliberate: this module used
 * to read `result.refused === true` off a body the session had already
 * flattened, which made the two real refusal shapes (prose from `fail()`, an
 * OBJECT from `refusalOrFail()`) score as ordinary successful steps. The
 * envelope is the one place the wire is interpreted, and it is interpreted
 * with the gates.ts detector.
 */

const GAP_NO_SOUND =
  "the call was refused before geometry existed, so soundness was never " +
  "measured — there is no verdict to report";
const GAP_NEVER_MEASURED =
  "no step in this episode reported the component";

/**
 * The honest reason `fidelity_signed` is absent on a SUCCESSFUL step.
 *
 * The kernel computes it and the api-server attaches it to a mutating op's
 * response at `body.perception.fidelity` (api-server/src/main.rs:1326-1336
 * `attach_fidelity`, block shape at main.rs:1247-1314), and roshera-mcp carries
 * it through VERBATIM (`perceptionFromBody`, core.ts — proven at the production
 * dispatch path by roshera-mcp/test/perception_fidelity.test.mjs).
 *
 * WHAT THIS REASON MAY AND MAY NOT SAY. It used to end "this absence means the
 * op attached none". The first CONCURRENT live run (8 episodes, 2026-08-13)
 * disproved that sentence with its own trajectories: five of the eight built
 * cylinders — an op that ALWAYS attaches a block (main.rs:4237) — and every one
 * of those five recorded this gap. The block existed; the client lost it. The
 * `part_id`s in those records say how: 97,97,97,97,98,101,101,101, four
 * episodes naming one part, because `GET /api/agent/parts` is served from the
 * ONE global live model for any request without an `X-Roshera-Part-Id` header
 * (api-server/src/part_mgr.rs:264,286-312) and the MCP never sends one. The
 * tool resolved another session's id, `perceive()` missed its
 * embedded-perception stash (matched BY ID, core.ts) and re-fetched the
 * read-side `/perception` endpoint, which has no fidelity producer at all.
 *
 * That client-side cause is designed out — `newestPartId` (core.ts) now returns
 * the id the op's own response named — but the lesson stands and is why this
 * text no longer commits to one cause: the trajectory records only that no
 * block arrived, and a reason cannot see further than the wire it is reading.
 * So it ENUMERATES the ways, source-cited, and asserts none of them.
 *
 * This constant is for the NO-BLOCK case ONLY. A block that arrived and simply
 * had no number in it is a different fact with a different, better reason —
 * the kernel's own — and `fidelityGapReason` below reports that one instead.
 *
 * A step whose `perception` arrived as PROSE is a different absence again, and
 * `soundnessOf` below names that one where it belongs.
 */
const GAP_NO_FIDELITY =
  "this step's perception carried no fidelity block, and this record cannot " +
  "tell which of the following put it there. The kernel measures fidelity and " +
  "the api-server attaches it at body.perception.fidelity (main.rs " +
  "attach_fidelity) for the four ops with a measurable request — cylinder, box, " +
  "revolve, loft. So EITHER no block was attached: the op is not one of those " +
  "four, or its report was empty and an empty report attaches nothing rather " +
  "than a block of zeros (main.rs:1330-1332), or the step was a verify_part, " +
  "whose own body (tools/perception.ts) is built from the read-side " +
  "/perception endpoint and never carries one. OR a block WAS attached and the " +
  "client did not carry it here: roshera-mcp reuses the op's embedded " +
  "perception only when it matches the part id the tool resolved, and on a " +
  "mismatch it re-fetches that same read-side /perception endpoint, which has " +
  "no fidelity producer — which is what happened to five of the eight " +
  "concurrent live episodes of 2026-08-13. What is certain is that NO measured " +
  "deviation was seen and none is being scored as 0";

/**
 * WHY there is no `fidelity_signed` on this step — distinguishing "no block
 * arrived" from "a block arrived carrying no number", and in the second case
 * QUOTING THE KERNEL instead of guessing.
 *
 * A gaps-only report is not an absent one: `FidelityReport::is_empty()` is true
 * only when the quantities list AND the gaps list are both empty
 * (geometry-engine/src/queries/fidelity.rs:215-217), so a report that measured
 * nothing but recorded WHY still gets attached (main.rs:1330-1334) — with
 * `fidelity_ok` omitted (main.rs:1276-1279), `worst: null` (main.rs:1296), and
 * every unmeasured quantity in `gaps` as `{name, reason}`
 * (fidelity.rs:137-140). That `reason` is the kernel's own account of why it
 * could not measure, which is strictly better than anything this module could
 * infer — and the old text both contradicted it ("the op attached none") and
 * threw it away.
 */
function fidelityGapReason(fid) {
  if (fid === null || typeof fid !== "object") return GAP_NO_FIDELITY;
  const op = typeof fid.op === "string" ? fid.op : "unnamed op";
  const gaps = Array.isArray(fid.gaps) ? fid.gaps : [];
  const stated = gaps
    .filter((g) => g && typeof g.name === "string" && typeof g.reason === "string")
    .map((g) => `${g.name}: ${g.reason}`);
  if (stated.length > 0) {
    return (
      `the ${op} fidelity block arrived and measured nothing comparable — the ` +
      `KERNEL's own stated reason(s): ${stated.join(" | ")}. Reported verbatim ` +
      `rather than paraphrased, and never scored as 0`
    );
  }
  // A block with neither a readable `worst` nor any stated gap. Say exactly
  // that, and hand on what did arrive so the shape can be inspected, rather
  // than asserting a cause.
  return (
    `the ${op} fidelity block arrived but carried no signed deviation to read ` +
    `(worst=${JSON.stringify(fid.worst ?? null)}, ` +
    `${Array.isArray(fid.quantities) ? fid.quantities.length : 0} quantities, ` +
    `${gaps.length} gaps) and stated no reason of its own — this module will ` +
    `not invent one`
  );
}

/** Both real shapes for the ambient soundness verdict, in one place. */
function soundnessOf(data) {
  // 1. `okp()` in `cert` mode: the full perception OBJECT (core.ts:929-930),
  //    whose `sound` is the authoritative verdict (core.ts:338-339).
  const p = data?.perception;
  if (typeof p?.sound === "boolean") return { sound: p.sound, gap: null };
  // 2. `verify_part` builds its own body with `sound` at the TOP LEVEL
  //    (tools/perception.ts:206) — there is no `perception` wrapper on it.
  if (typeof data?.sound === "boolean") return { sound: data.sound, gap: null };
  // 3. A STRING perception is either the token-diet compact verdict
  //    (core.ts:923-926) or `perceptionField`'s typed unavailability note
  //    (core.ts:822-825). Both are prose; parsing prose into a boolean
  //    verdict is precisely the guess this environment refuses to make, so
  //    the string is reported verbatim as the reason.
  if (typeof p === "string") {
    return {
      sound: null,
      gap:
        `the op reported its verdict as PROSE, not a boolean: ${JSON.stringify(p)} ` +
        `— spawn the session with ROSHERA_AMBIENT_PERCEPTION=cert to get the ` +
        `perception object (mcp_session.mjs pins it; this result did not come ` +
        `from a session that did)`,
    };
  }
  return { sound: null, gap: GAP_NEVER_MEASURED };
}

/** One step's reward, read off an `mcp_session.readToolResult` envelope. */
export function rewardFromResult(envelope) {
  const components = {};
  const gaps = [];

  // ── the call was REFUSED ────────────────────────────────────────────────
  // `envelope.refusal` is the gates.ts detector's verdict, so all three real
  // refusal shapes land here: `refused:true`, `refused:<object>`, and an
  // isError result carrying the REFUSED marker.
  if (envelope?.refusal) {
    // `||` rather than `??`: a refusal must always carry a NON-EMPTY gate
    // name, because `mergeFinal` counts refusals by testing that this field
    // is a string, and an empty string would be indistinguishable from a
    // named gate to a human reading the trajectory.
    components.refused = envelope.refusal.gate || "unnamed_gate";
    gaps.push({ name: "sound", reason: GAP_NO_SOUND });
    gaps.push({ name: "fidelity_signed", reason: GAP_NO_SOUND });
    return { components, gaps };
  }

  // ── the call FAILED without being a typed refusal ───────────────────────
  // An isError result, or a body that is not an object, is not a successful
  // step and must never be scored as one. It is not a refusal either — no
  // gate named it — so `refused` stays the determinate `null` and the failure
  // gets its own named component rather than hiding inside the refusal count.
  if (envelope?.is_error === true || envelope?.data === null || envelope?.data === undefined) {
    const why = envelope?.text ?? envelope?.parse_error ?? "no result was returned";
    components.call_failed = why;
    components.refused = null;
    if (envelope?.rate_limited === true) components.rate_limited = true;
    gaps.push({ name: "sound", reason: `the call failed, so soundness was never measured: ${why}` });
    gaps.push({ name: "fidelity_signed", reason: `the call failed, so fidelity was never measured: ${why}` });
    return { components, gaps };
  }

  const data = envelope.data;
  const { sound, gap } = soundnessOf(data);
  if (sound === null) {
    gaps.push({ name: "sound", reason: gap });
  } else {
    components.sound = sound;
  }

  const fid = data?.perception?.fidelity;
  const signed = fid?.worst?.signed_relative_deviation;
  if (typeof signed === "number" && Number.isFinite(signed)) {
    components.fidelity_signed = signed;
  } else {
    // Two different absences, two different reasons — see `fidelityGapReason`.
    gaps.push({ name: "fidelity_signed", reason: fidelityGapReason(fid ?? null) });
  }

  // A determinate tri-state, not an unmeasured absence: a call either was or
  // wasn't refused, and that is always knowable — so `refused` never belongs
  // in `gaps`, and `null` here (rather than omitting the key) is what lets
  // `mergeFinal` tell "not refused" apart from "field never reported".
  components.refused = null;
  return { components, gaps };
}

/**
 * The episode's terminal reading — per component, not a sum.
 *
 * `fidelity_signed` takes the WORST (largest magnitude) deviation seen, not
 * the last and not the mean: a mean would let one good step hide a bad one,
 * which is exactly the "certified sound at 9.97%" failure this signal exists
 * to surface.
 */
export function mergeFinal(rewards) {
  const components = { refusals: 0, call_failures: 0 };
  const gaps = [];

  let sawSound = false;
  let worst = null;
  for (const r of rewards) {
    // Count on a determinate test, not truthiness: `refused` is either a
    // (non-empty, per rewardFromResult) gate string or `null`, and a
    // falsy-but-real gate name must never make a genuine refusal vanish.
    if (typeof r.components.refused === "string") components.refusals += 1;
    // A failed call is counted separately from a refusal, because they are
    // different facts: a refusal is the kernel holding a line, a failure is
    // the call not landing. Collapsing them would make "the moat held N
    // times" unreadable.
    if (typeof r.components.call_failed === "string") components.call_failures += 1;
    if (typeof r.components.sound === "boolean") {
      components.sound = r.components.sound;
      sawSound = true;
    }
    const f = r.components.fidelity_signed;
    if (typeof f === "number" && (worst === null || Math.abs(f) > Math.abs(worst))) {
      worst = f;
    }
  }
  if (worst !== null) components.fidelity_signed = worst;

  if (!sawSound) gaps.push({ name: "sound", reason: GAP_NEVER_MEASURED });
  if (worst === null) gaps.push({ name: "fidelity_signed", reason: GAP_NEVER_MEASURED });
  return { components, gaps };
}
