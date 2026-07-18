/** The AGENT-EVAL-α corpus, in execution order. */
import gear from "./01-gear.mjs";
import nozzle from "./02-nozzle.mjs";
import injector from "./03-injector.mjs";
import bulkhead from "./04-bulkhead.mjs";
import pocketBlock from "./05-pocket-block.mjs";
import hubFlangeGdt from "./06-hub-flange-gdt.mjs";
import stepRoundtrip from "./07-step-roundtrip.mjs";
import saddleHonesty from "./08-saddle-honesty.mjs";
import drawingPerf from "./09-drawing-perf.mjs";
import assemblyKinematics from "./10-assembly-kinematics.mjs";
import sketchCertifiedBore from "./11-sketch-certified-bore.mjs";
import massPropertiesHonesty from "./12-mass-properties-honesty.mjs";
import coincidentFaceRobustness from "./13-coincident-face-robustness.mjs";
import quadricSsiHonesty from "./14-quadric-ssi-honesty.mjs";
import drawingComprehension from "./15-drawing-comprehension.mjs";

export const scenarios = [
  gear,
  nozzle,
  injector,
  bulkhead,
  pocketBlock,
  hubFlangeGdt,
  stepRoundtrip,
  saddleHonesty,
  drawingPerf,
  assemblyKinematics,
  sketchCertifiedBore,
  massPropertiesHonesty,
  coincidentFaceRobustness,
  quadricSsiHonesty,
  drawingComprehension,
];
