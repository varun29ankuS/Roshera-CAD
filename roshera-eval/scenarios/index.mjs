/** The AGENT-EVAL-α v1 corpus, in execution order. */
import gear from "./01-gear.mjs";
import nozzle from "./02-nozzle.mjs";
import injector from "./03-injector.mjs";
import bulkhead from "./04-bulkhead.mjs";
import pocketBlock from "./05-pocket-block.mjs";
import hubFlangeGdt from "./06-hub-flange-gdt.mjs";
import stepRoundtrip from "./07-step-roundtrip.mjs";
import saddleHonesty from "./08-saddle-honesty.mjs";
import drawingPerf from "./09-drawing-perf.mjs";

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
];
