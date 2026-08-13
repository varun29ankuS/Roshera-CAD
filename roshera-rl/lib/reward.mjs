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
 */

const GAP_NO_FIDELITY =
  "the op returned no fidelity block: this op class has no measurable " +
  "requested parameters, or the certificate was skipped (fast:true)";
const GAP_NO_SOUND =
  "the call was refused before geometry existed, so soundness was never " +
  "measured — there is no verdict to report";
const GAP_NEVER_MEASURED =
  "no step in this episode reported the component";

/** One step's reward, read off a parsed MCP tool result. */
export function rewardFromResult(result) {
  const components = {};
  const gaps = [];

  if (result?.refused === true) {
    components.refused = result.gate ?? "unnamed_gate";
    gaps.push({ name: "sound", reason: GAP_NO_SOUND });
    gaps.push({ name: "fidelity_signed", reason: GAP_NO_SOUND });
    return { components, gaps };
  }

  const perception = result?.perception;
  if (typeof perception?.sound === "boolean") {
    components.sound = perception.sound;
  } else {
    gaps.push({ name: "sound", reason: GAP_NEVER_MEASURED });
  }

  const signed = perception?.fidelity?.worst?.signed_relative_deviation;
  if (typeof signed === "number" && Number.isFinite(signed)) {
    components.fidelity_signed = signed;
  } else {
    gaps.push({ name: "fidelity_signed", reason: GAP_NO_FIDELITY });
  }

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
  const components = { refusals: 0 };
  const gaps = [];

  let sawSound = false;
  let worst = null;
  for (const r of rewards) {
    if (r.components.refused) components.refusals += 1;
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
