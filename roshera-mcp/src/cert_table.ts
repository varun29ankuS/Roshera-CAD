/**
 * The full certificate as a table: one check per line.
 *
 * Varun, 2026-11-26: certificates shown "line by line with a right tick at
 * the end with icons ... so that the whole thing looks very professional",
 * the same layout as the app's Soundness card (CertRow). `verify_part` leads
 * its answer with this table, above the machine-readable JSON:
 *
 *   Soundness · part 7                                  SOUND
 *   ──────────────────────────────────────────────────────────
 *   ▣  B-Rep valid               412 faces              ✓
 *   ◉  Watertight                0 open edges           ✓
 *   ...
 *
 * A check that did not run reads "not run" (never a tick: not run is not a
 * pass); a check with nothing to judge on this part reads "n/a".
 */

/** The verdict of one check: proven, failed, nothing to judge, or not run. */
type Verdict = "pass" | "fail" | "na" | "not_run";

const MARK: Record<Verdict, string> = {
  pass: "✓",
  fail: "✗",
  na: "n/a",
  not_run: "not run",
};

const toVerdict = (v: unknown): Verdict => {
  if (v === true || v === "consistent") return "pass";
  if (v === false || v === "inconsistent") return "fail";
  if (v === "not_applicable") return "na";
  return "not_run";
};

const count = (n: unknown, what: string): string | undefined =>
  typeof n === "number" && Number.isFinite(n) ? `${n.toLocaleString("en-US")} ${what}` : undefined;

/** The checks in reading order: icon, full name, the field(s) holding the
 *  verdict, and the measured evidence printed beside it. */
const CHECKS: ReadonlyArray<{
  icon: string;
  name: string;
  verdict: (o: any) => unknown;
  value?: (o: any) => string | undefined;
}> = [
  {
    icon: "▣",
    name: "B-Rep valid",
    verdict: (o) => o.brep_valid,
    value: (o) => count(o.face_count ?? o.dims?.face_count, "faces"),
  },
  {
    icon: "◉",
    name: "Watertight",
    verdict: (o) => o.brep_watertight ?? o.watertight,
    value: (o) => count(o.open_edges ?? o.cert?.open_edges, "open edges"),
  },
  {
    icon: "⬡",
    name: "Manifold",
    verdict: (o) => o.manifold,
    value: (o) => count(o.nonmanifold_edges ?? o.cert?.nonmanifold_edges, "non-manifold"),
  },
  {
    icon: "↗",
    name: "Shells face outward",
    verdict: (o) => o.shells_outward,
    value: (o) =>
      Array.isArray(o.misoriented_shells) && o.misoriented_shells.length
        ? `${o.misoriented_shells.length} turned inward`
        : undefined,
  },
  { icon: "⨯", name: "No self-intersection", verdict: (o) => o.self_intersection_free },
  { icon: "⚙", name: "Construction consistent", verdict: (o) => o.construction_consistent },
  { icon: "▦", name: "Tessellation clean", verdict: (o) => o.tessellation_clean },
  { icon: "◇", name: "Mesh quality", verdict: (o) => o.mesh_quality_clean },
  { icon: "◎", name: "Dual-view consistent", verdict: (o) => o.eyes_consistent },
];

const NAME_W = 25;
const VALUE_W = 20;

/** Pad by code points, so a glyph counts as one column. */
const pad = (s: string, w: number): string => s + " ".repeat(Math.max(0, w - [...s].length));

/**
 * Render a `verify_part` answer (or any perception carrying the certificate
 * conjuncts) as the line-by-line table. Pure: no I/O.
 */
export function certificateTable(o: any): string {
  const who = o?.part ? String(o.part) : o?.part_id != null ? `part ${o.part_id}` : "part";
  const overall = o?.sound === true ? "SOUND" : o?.sound === false ? "UNSOUND" : "NO VERDICT";
  const rows = CHECKS.map((c) => {
    const v = toVerdict(c.verdict(o ?? {}));
    const value = c.value?.(o ?? {}) ?? "";
    return `${c.icon}  ${pad(c.name, NAME_W)}${pad(value, VALUE_W)}${MARK[v]}`;
  });
  const width = Math.max(...rows.map((r) => [...r].length), 3 + NAME_W + VALUE_W + 1);
  const title = `Soundness · ${who}`;
  const header = pad(title, width - overall.length) + overall;
  const rule = "─".repeat(width);
  const facts: string[] = [];
  if (typeof o?.euler_characteristic === "number") facts.push(`Euler characteristic χ = ${o.euler_characteristic}`);
  return [header, rule, ...rows, ...(facts.length ? [rule, ...facts] : [])].join("\n");
}
