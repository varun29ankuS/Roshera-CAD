"""Before/after judge for drawing changes.

Every change to the drawing pipeline is run over the whole bench (49
CADGenBench parts + 11 NIST parts, `bench.py`) and judged here, part by part,
against the run before it. A change that makes ANY part's sheet worse fails,
however much it improves the rest -- the way a drawing kit's examples must
stay byte-identical unless someone looks and accepts the change.

A part is WORSE when, compared with before:
  * its sheet no longer draws (stage was "done", now is not), or it vanished;
  * the certificate lost soundness, or gained stale / dangling dimensions;
  * it has more layout errors, or more dimensions left off the sheet;
  * its views went from filling the sheet to under-filling it (sheet
    utilization from at least UNDERFILLED to below it: the quality check's
    `SheetUnderutilized` threshold);
  * it got much slower (over 1.5x AND over 5 s more).
A trade (fewer errors but more dimensions missing) is worse: a regression is
not paid for by an improvement somewhere else on the same sheet.
BETTER is strictly fewer errors or missing dimensions (or completeness gained,
or the views fill clearly more of the sheet -- utilization up by at least
UTIL_GAIN) with nothing worse; a fuller sheet with one new error is WORSE,
never better. Anything else is SAME. Every row also reports the sheet size,
the scale and the utilization before and after.

Usage:
  python -I regress.py --before runs/baseline --after runs/final [--md out.md]
Exit 0 when no part is worse, 1 otherwise.
"""
import argparse
import json
import os
import sys

# The quality check's `SheetUnderutilized` threshold (verify.rs MIN_UTILIZATION).
UNDERFILLED = 0.10
# A utilization gain this large is an improvement on its own.
UTIL_GAIN = 0.02
# Grades are written to 3+ decimals; a difference below this is float noise.
GRADE_EPS = 1e-6


def _load(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def _facts(s):
    q = s.get("quality") or {}
    c = s.get("certificate") or {}
    counts = c.get("counts") or {}
    sheet = s.get("sheet") or {}
    views = sheet.get("views") or []
    util = q.get("sheet_utilization")
    sp = s.get("spaces") or {}
    return {
        # the space map (drafting craft): the grade is the WORST space's score;
        # None on sheets from before the map existed
        "grade": float(sp["grade"]) if sp.get("grade") is not None else None,
        "below_bar": int(sp["below_bar"]) if sp.get("below_bar") is not None else None,
        "drawn": s.get("stage") == "done",
        "errors": int(q.get("errors") or 0),
        "not_dim": int(counts.get("not_dimensioned") or 0),
        "stale": int(counts.get("stale") or 0) + int(counts.get("dangling") or 0),
        "complete": bool(c.get("complete")),
        "sound": c.get("sound", True) is not False,
        "ms": int(sheet.get("ms") or 0),
        "size": sheet.get("size") or "-",
        "scale": views[0].get("scale") if views else None,
        "util": float(util) if util is not None else None,
    }


def judge(before, after):
    """{'verdict': 'better'|'same'|'worse', 'why': [..], 'before': facts, 'after': facts}."""
    return judge_facts(_facts(before), _facts(after))


def judge_facts(b, a):
    """`judge` on facts already read (a run's summaries, or the floors file)."""
    worse, better = [], []
    if b["drawn"] and not a["drawn"]:
        worse.append("sheet no longer draws")
    if b["sound"] and not a["sound"]:
        worse.append("certificate lost soundness")
    if a["stale"] > b["stale"]:
        worse.append(f"stale/dangling {b['stale']} -> {a['stale']}")
    if a["errors"] > b["errors"]:
        worse.append(f"layout errors {b['errors']} -> {a['errors']}")
    elif a["errors"] < b["errors"]:
        better.append(f"layout errors {b['errors']} -> {a['errors']}")
    if a["not_dim"] > b["not_dim"]:
        worse.append(f"not dimensioned {b['not_dim']} -> {a['not_dim']}")
    elif a["not_dim"] < b["not_dim"]:
        better.append(f"not dimensioned {b['not_dim']} -> {a['not_dim']}")
    if b["complete"] and not a["complete"]:
        worse.append("no longer complete")
    elif a["complete"] and not b["complete"]:
        better.append("now complete")
    if b["util"] is not None and a["util"] is not None:
        if b["util"] >= UNDERFILLED > a["util"]:
            worse.append(f"utilization {b['util']:.3f} -> {a['util']:.3f} (under-filled)")
        elif a["util"] - b["util"] >= UTIL_GAIN:
            better.append(f"utilization {b['util']:.3f} -> {a['util']:.3f}")
    if b["grade"] is not None:
        if a["grade"] is None:
            if a["drawn"]:
                worse.append("space map missing")
        else:
            if a["grade"] < b["grade"] - GRADE_EPS:
                worse.append(f"grade {b['grade']:.3f} -> {a['grade']:.3f}")
            elif a["grade"] > b["grade"] + GRADE_EPS:
                better.append(f"grade {b['grade']:.3f} -> {a['grade']:.3f}")
            if a["below_bar"] > b["below_bar"]:
                worse.append(f"spaces below bar {b['below_bar']} -> {a['below_bar']}")
            elif a["below_bar"] < b["below_bar"]:
                better.append(f"spaces below bar {b['below_bar']} -> {a['below_bar']}")
    if a["ms"] > 1.5 * b["ms"] and a["ms"] - b["ms"] > 5000:
        worse.append(f"sheet time {b['ms']} -> {a['ms']} ms")
    if not b["drawn"] and a["drawn"]:
        better.append("sheet now draws")
    verdict = "worse" if worse else ("better" if better else "same")
    return {"verdict": verdict, "why": worse + better, "before": b, "after": a}


def compare(before_dir, after_dir):
    """Judge every part of `before_dir` against `after_dir`.
    Returns (rows sorted by id, ids of parts that are worse)."""
    rows, worse = [], []
    for pid in sorted(os.listdir(before_dir)):
        b = _load(os.path.join(before_dir, pid, "summary.json"))
        if b is None:
            continue
        a = _load(os.path.join(after_dir, pid, "summary.json"))
        if a is None:
            v = {"verdict": "worse", "why": ["no result after the change"], "before": _facts(b), "after": None}
        else:
            v = judge(b, a)
        rows.append({"id": pid, **v})
        if v["verdict"] == "worse":
            worse.append(pid)
    return rows, worse


FLOORS_VERSION = 1


def write_floors(run_dir, path):
    """Snapshot every part's facts from an observed run into `path` (checked
    in; runs/ is not). Floors encode what was measured, never an aspiration."""
    parts = {}
    for pid in sorted(os.listdir(run_dir)):
        s = _load(os.path.join(run_dir, pid, "summary.json"))
        if s is not None:
            parts[pid] = _facts(s)
    with open(path, "w", encoding="utf-8") as f:
        json.dump({"version": FLOORS_VERSION, "from_run": os.path.basename(os.path.normpath(run_dir)),
                   "parts": parts}, f, indent=1, sort_keys=True)
        f.write("\n")
    return parts


def check_floors(path, after_dir):
    """Judge `after_dir` against the floors file with the before/after rules.
    Returns (rows sorted by id, ids of parts that fell below their floor)."""
    floors = _load(path)
    if not floors or floors.get("version") != FLOORS_VERSION:
        raise SystemExit(f"floors file {path} missing or not version {FLOORS_VERSION}")
    rows, worse = [], []
    for pid, b in sorted(floors["parts"].items()):
        a = _load(os.path.join(after_dir, pid, "summary.json"))
        if a is None:
            v = {"verdict": "worse", "why": ["no result after the change"], "before": b, "after": None}
        else:
            v = judge_facts(b, _facts(a))
        rows.append({"id": pid, **v})
        if v["verdict"] == "worse":
            worse.append(pid)
    return rows, worse


def _scale(s):
    """An ISO 5455 scale as a drawing states it: 2:1, 1:1, 1:2."""
    if not s:
        return "-"
    return f"{s:g}:1" if s >= 1 else f"1:{1 / s:g}"


def _pct(u):
    return "-" if u is None else f"{100 * u:.1f}%"


def markdown(rows, worse):
    n = {k: sum(1 for r in rows if r["verdict"] == k) for k in ("better", "same", "worse")}
    out = [
        f"# Drawing before/after: {n['better']} better, {n['same']} same, {n['worse']} worse "
        f"({len(rows)} parts)",
        "",
        "| Part | Verdict | Sheet | Scale | Utilization | Errors | Not dimensioned | Complete | Sheet ms | Why |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        b, a = r["before"], r["after"] or {}
        out.append(
            f"| {r['id']} | {r['verdict']} | {b['size']} -> {a.get('size', '-')} | "
            f"{_scale(b['scale'])} -> {_scale(a.get('scale'))} | "
            f"{_pct(b['util'])} -> {_pct(a.get('util'))} | "
            f"{b['errors']} -> {a.get('errors', '-')} | "
            f"{b['not_dim']} -> {a.get('not_dim', '-')} | {b['complete']} -> {a.get('complete', '-')} | "
            f"{b['ms']} -> {a.get('ms', '-')} | {'; '.join(r['why'])} |"
        )
    if worse:
        out += ["", f"WORSE: {', '.join(worse)}"]
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", default="", help="an earlier run directory")
    ap.add_argument("--floors", default="", help="the checked-in floors file (instead of --before)")
    ap.add_argument("--after", required=True, help="the run to judge")
    ap.add_argument("--write-floors", default="",
                    help="snapshot --after into this floors file and judge nothing")
    ap.add_argument("--md", default="")
    args = ap.parse_args()
    if args.write_floors:
        parts = write_floors(args.after, args.write_floors)
        sys.stdout.write(f"floors: {len(parts)} parts from {args.after} -> {args.write_floors}\n")
        return 0
    if bool(args.before) == bool(args.floors):
        ap.error("give exactly one of --before or --floors")
    rows, worse = check_floors(args.floors, args.after) if args.floors else compare(args.before, args.after)
    text = markdown(rows, worse)
    if args.md:
        with open(args.md, "w", encoding="utf-8") as f:
            f.write(text)
    sys.stdout.write(text)
    return 1 if worse else 0


if __name__ == "__main__":
    sys.exit(main())
