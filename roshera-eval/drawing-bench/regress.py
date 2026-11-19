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
  * it got much slower (over 1.5x AND over 5 s more).
A trade (fewer errors but more dimensions missing) is worse: a regression is
not paid for by an improvement somewhere else on the same sheet.
BETTER is strictly fewer errors or missing dimensions (or completeness gained)
with nothing worse; anything else is SAME.

Usage:
  python -I regress.py --before runs/baseline --after runs/final [--md out.md]
Exit 0 when no part is worse, 1 otherwise.
"""
import argparse
import json
import os
import sys


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
    return {
        "drawn": s.get("stage") == "done",
        "errors": int(q.get("errors") or 0),
        "not_dim": int(counts.get("not_dimensioned") or 0),
        "stale": int(counts.get("stale") or 0) + int(counts.get("dangling") or 0),
        "complete": bool(c.get("complete")),
        "sound": c.get("sound", True) is not False,
        "ms": int(sheet.get("ms") or 0),
    }


def judge(before, after):
    """{'verdict': 'better'|'same'|'worse', 'why': [..], 'before': facts, 'after': facts}."""
    b, a = _facts(before), _facts(after)
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


def markdown(rows, worse):
    n = {k: sum(1 for r in rows if r["verdict"] == k) for k in ("better", "same", "worse")}
    out = [
        f"# Drawing before/after: {n['better']} better, {n['same']} same, {n['worse']} worse "
        f"({len(rows)} parts)",
        "",
        "| Part | Verdict | Errors | Not dimensioned | Complete | Sheet ms | Why |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        b, a = r["before"], r["after"] or {}
        out.append(
            f"| {r['id']} | {r['verdict']} | {b['errors']} -> {a.get('errors', '-')} | "
            f"{b['not_dim']} -> {a.get('not_dim', '-')} | {b['complete']} -> {a.get('complete', '-')} | "
            f"{b['ms']} -> {a.get('ms', '-')} | {'; '.join(r['why'])} |"
        )
    if worse:
        out += ["", f"WORSE: {', '.join(worse)}"]
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", required=True)
    ap.add_argument("--after", required=True)
    ap.add_argument("--md", default="")
    args = ap.parse_args()
    rows, worse = compare(args.before, args.after)
    text = markdown(rows, worse)
    if args.md:
        with open(args.md, "w", encoding="utf-8") as f:
            f.write(text)
    sys.stdout.write(text)
    return 1 if worse else 0


if __name__ == "__main__":
    sys.exit(main())
