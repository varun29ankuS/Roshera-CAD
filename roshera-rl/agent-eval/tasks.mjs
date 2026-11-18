// Agent-eval tasks: parts specified the way a drawing would state them, each
// with checks the harness computes INDEPENDENTLY of the agent (closed-form
// volume, bounding box) plus the kernel's own soundness certificate.
//
// A task passes only when the agent's final model holds exactly one solid,
// that solid certifies sound, its volume is within `volumeTol` (relative) of
// the closed form, and its bounding-box extents match the spec (sorted, so
// the agent may orient the part however it likes) within 0.01 mm.

const PI = Math.PI;
const cyl = (d, h) => PI * (d / 2) ** 2 * h;

export const TASKS = [
  {
    id: "plate-4-holes",
    spec:
      "A rectangular plate 100 mm long, 60 mm wide and 8 mm thick, with four " +
      "6.6 mm diameter through holes. Each hole centre is 10 mm in from both " +
      "adjacent edges.",
    volume: 100 * 60 * 8 - 4 * cyl(6.6, 8),
    extents: [100, 60, 8],
  },
  {
    id: "flanged-bushing",
    spec:
      "A flanged bushing on one axis: a flange 50 mm in diameter and 6 mm " +
      "thick, and a hub 30 mm in diameter rising 20 mm above the flange (26 mm " +
      "overall). A 16 mm diameter bore runs through the whole part on the axis.",
    volume: cyl(50, 6) + cyl(30, 20) - cyl(16, 26),
    extents: [50, 50, 26],
  },
  {
    id: "counterbored-block",
    spec:
      "A 40 x 40 x 20 mm block. In the centre of one 40 x 40 face, a counterbored " +
      "hole for an M6 cap screw: 6.6 mm through hole, counterbore 11 mm diameter " +
      "and 6 mm deep.",
    volume: 40 * 40 * 20 - cyl(6.6, 20 - 6) - cyl(11, 6),
    extents: [40, 40, 20],
  },
  {
    id: "stepped-shaft",
    spec:
      "A stepped shaft on one axis: 20 mm diameter for 40 mm, then 14 mm " +
      "diameter for 30 mm, then 10 mm diameter for 20 mm (90 mm overall).",
    volume: cyl(20, 40) + cyl(14, 30) + cyl(10, 20),
    extents: [20, 20, 90],
  },
  {
    id: "pipe-flange",
    spec:
      "A pipe flange: a disk 120 mm in diameter and 12 mm thick with a 60 mm " +
      "central bore, and six 10 mm bolt holes equally spaced on a 90 mm pitch " +
      "circle diameter, all through.",
    volume: cyl(120, 12) - cyl(60, 12) - 6 * cyl(10, 12),
    extents: [120, 120, 12],
  },
  {
    id: "open-box",
    spec:
      "An open-top box: outside 80 x 60 x 40 mm (40 mm tall), walls and floor " +
      "3 mm thick, no lid.",
    volume: 80 * 60 * 40 - 74 * 54 * 37,
    extents: [80, 60, 40],
  },
  {
    id: "slotted-plate",
    spec:
      "A plate 80 x 40 x 6 mm with one through slot in its centre, the slot's " +
      "long axis along the plate's length: slot 30 mm long overall and 8 mm " +
      "wide, with full-round ends (radius 4 mm).",
    volume: 80 * 40 * 6 - (22 * 8 + PI * 4 ** 2) * 6,
    extents: [80, 40, 6],
  },
  {
    id: "hex-nut-blank",
    spec:
      "A hexagonal nut blank: 13 mm across the flats, 6.5 mm thick, with an " +
      "8 mm diameter through hole on its axis (no thread).",
    volume: (Math.sqrt(3) / 2) * 13 ** 2 * 6.5 - cyl(8, 6.5),
    // Across corners = 13 / cos(30 deg); either in-plane orientation is fine,
    // so only the thickness and the across-flats are checked exactly.
    extents: null,
    extentsAnyOf: [
      [13 / Math.cos(PI / 6), 13, 6.5],
    ],
  },
  {
    id: "l-bracket-filleted",
    spec:
      "An L-bracket 30 mm wide: a base leg 60 mm long and an upright leg 40 mm " +
      "tall (both measured to the outside), both 5 mm thick, with a 5 mm radius " +
      "fillet on the inside corner where the legs meet.",
    volume: (60 * 5 + 35 * 5 + (25 - (PI * 25) / 4)) * 30,
    extents: [60, 40, 30],
    volumeTol: 0.01,
  },
  {
    id: "spacer-ring-chamfered",
    spec:
      "A spacer ring: outside diameter 40 mm, bore 20 mm, 10 mm thick, with a " +
      "1 mm x 45 degree chamfer on both outside edges (the outer circular " +
      "edges) only.",
    // Each 1x45 chamfer on the outer rim removes a triangular ring of
    // cross-section 0.5 mm^2 at radius ~ 20 - 1/3.
    volume: cyl(40, 10) - cyl(20, 10) - 2 * 0.5 * 2 * PI * (20 - 1 / 3),
    extents: [40, 40, 10],
    volumeTol: 0.005,
  },
];

export function promptFor(task) {
  return (
    "You are building a part in Roshera CAD using only the roshera tools.\n\n" +
    `Part: ${task.spec}\n\n` +
    "All dimensions are millimetres. Placement and orientation are up to you. " +
    "Build it as ONE solid, check it with the kernel's certificate, and fix " +
    "anything that is wrong. When it is done, reply with one line: " +
    "DONE <part id>, or GAVE UP <reason> if you cannot build it."
  );
}
