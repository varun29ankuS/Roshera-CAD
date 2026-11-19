#!/usr/bin/env python3
"""Score a Roshera sheet against a reference-dimension list.

usage: python -I score.py <summary.json> <refs/ID.json> [--out score.json]
                          [--tol-rel 0.005] [--tol-abs 0.05]

`summary.json` is what the `drawing_bench` example writes: every dimension AS
INKED with its kind, value and faces, and the hole table. Scoring reads that
structured record, not the SVG text, so a number in the general-tolerance
note or the title block can never be mistaken for a dimension.

refs/ID.json (extracted by looking at the reference drawing, once per part):
{
  "id": "101",
  "source": "who extracted it and how",
  "views": ["front","top","right","iso","section","detail"],
  "dims":  [{"kind":"linear|diameter|radius|angle|chamfer","value":97.0,"count":1,"feature":"overall width"}],
  "holes": [{"dia":12.0,"count":4,"thru":true,"cbore_dia":20.0,"csink_angle":90.0,"thread":"M12x1.75"}],
  "notes": ["BREAK EDGES"]
}

What is matched, and how strictly (this replaces the first version, which
matched on value alone, searched hole diameters as substrings -- 12 matched
112 -- and looked for view names in all the text on the sheet):

* A reference dimension is FOUND only by an inked dimension of the same kind
  class whose value agrees within max(tol_abs, tol_rel * value). Each inked
  dimension satisfies at most one reference dimension (one-to-one), and a
  reference "count" of n needs n inked dimensions or one counted callout.
* A reference dimension whose value is on the sheet under ANOTHER kind is
  reported as `wrong_kind` and does not count as found.
* Recall = found / reference dimensions. Precision = inked dimensions that
  satisfied a reference dimension / inked dimensions. Both are reported:
  recall alone rewards over-dimensioning.
* A hole is found by a hole-table row or hole callout whose DIAMETER agrees
  numerically; its counterbore / countersink / thread claims are each checked
  against that same row or callout, and a missing one is named.
* View kinds are compared with the sheet's own view list.

What it cannot do: it does not know WHICH feature an inked dimension is on
beyond its kind and value (the reference names features in free text). Two
different 12 mm steps are told apart only by needing two inked 12s. Legibility,
attachment and layout are the looked-at pass (contact sheets).
"""
import json
import re
import sys

KIND_CLASS = {
    "extent": "linear", "length": "linear", "position": "linear", "location": "linear",
    "pitch": "linear", "diameter": "diameter", "pcd": "diameter", "hole_callout": "diameter",
    "radius": "radius", "fillet": "radius", "angle": "angle", "chamfer": "chamfer",
}
NUMBER = re.compile(r"(\d+(?:[.,]\d+)?)")


def close(a, b, rel, ab):
    return abs(a - b) <= max(ab, rel * max(abs(a), abs(b)))


def count_of(label):
    m = re.match(r"^\s*(\d+)\s*[xX×]", label or "")
    return int(m.group(1)) if m else 1


def numbers(text):
    return [float(n.replace(",", ".")) for n in NUMBER.findall(text or "")]


def score(summary, ref, rel, ab):
    sheet = summary.get("sheet", {}) if summary.get("stage") == "done" else {}
    inked = []
    unknown = {}
    for d in sheet.get("inked", []):
        # A kind this scorer does not know is counted and reported, and
        # matches nothing: guessing "linear" would score it as a length.
        cls = KIND_CLASS.get(d["kind"])
        if cls is None:
            unknown[d["kind"]] = unknown.get(d["kind"], 0) + 1
            cls = "unknown:" + d["kind"]
        inked.append({"cls": cls, "value": float(d["value"]),
                      "left": count_of(d.get("label")), "label": d.get("label"), "used": False})
    res = {"id": ref.get("id"), "built": bool(sheet)}

    # ── dimensions ────────────────────────────────────────────────────────
    found, wrong_kind, missing = [], [], []
    for r in ref.get("dims", []):
        need = int(r.get("count", 1))
        got = 0
        for o in inked:
            if got >= need:
                break
            if o["cls"] == r["kind"] and o["left"] > 0 and close(o["value"], r["value"], rel, ab):
                take = min(o["left"], need - got)
                o["left"] -= take
                o["used"] = True
                got += take
        if got >= need:
            found.append(r)
        elif any(close(o["value"], r["value"], rel, ab) and o["cls"] != r["kind"] for o in inked):
            wrong_kind.append(r)
        else:
            missing.append({**r, "found_count": got} if got else r)
    n_ref = len(ref.get("dims", []))
    used = sum(1 for o in inked if o["used"])
    res["dims"] = {
        "reference": n_ref, "found": len(found), "wrong_kind": wrong_kind, "missing": missing,
        "inked": len(inked), "inked_matching_a_reference": used, "unknown_kinds": unknown,
        "recall": round(len(found) / n_ref, 3) if n_ref else None,
        "precision": round(used / len(inked), 3) if inked else None,
    }

    # ── holes ─────────────────────────────────────────────────────────────
    statements = [{"dia": float(h["diameter_mm"]), "text": "%s %s" % (h.get("size", ""), h.get("depth", ""))}
                  for h in sheet.get("hole_table", [])]
    statements += [{"dia": float(d["value"]), "text": d.get("label", "")}
                   for d in sheet.get("inked", []) if d["kind"] in ("hole_callout", "diameter")]
    hole_missing = []
    for h in ref.get("holes", []):
        hits = [s for s in statements if close(s["dia"], h["dia"], rel, ab)]
        if not hits:
            hole_missing.append({**h, "why": "no row or callout of that diameter"})
            continue
        text = " ".join(s["text"] for s in hits)
        nums = numbers(text)
        problems = []
        if h.get("count", 1) > 1 and len(hits) < h["count"] and not any(count_of(s["text"]) >= h["count"] for s in hits):
            problems.append("count %d not stated" % h["count"])
        if h.get("thru") and "THRU" not in text.upper():
            problems.append("THRU not stated")
        if h.get("cbore_dia") and not ("⌴" in text and any(close(n, h["cbore_dia"], rel, ab) for n in nums)):
            problems.append("counterbore not stated")
        if h.get("csink_angle") and "⌵" not in text:
            problems.append("countersink not stated")
        if h.get("thread") and h["thread"].lower().replace(" ", "") not in text.lower().replace(" ", ""):
            problems.append("thread not stated")
        if problems:
            hole_missing.append({**h, "why": "; ".join(problems)})
    n_holes = len(ref.get("holes", []))
    res["holes"] = {"reference": n_holes, "incomplete_or_missing": hole_missing,
                    "recall": round((n_holes - len(hole_missing)) / n_holes, 3) if n_holes else None}

    # ── view kinds ────────────────────────────────────────────────────────
    names = " ".join(v["name"].lower() for v in sheet.get("views", []))
    has = {"front": "front" in names, "top": "top" in names, "right": "right" in names,
           "left": "left" in names, "bottom": "bottom" in names, "iso": "iso" in names,
           "section": bool(sheet.get("has_section")), "detail": "detail" in names, "aux": "aux" in names}
    wanted = ref.get("views", [])
    view_missing = [v for v in wanted if not has.get(v, False)]
    res["views"] = {"reference": wanted, "missing": view_missing,
                    "coverage": round((len(wanted) - len(view_missing)) / len(wanted), 3) if wanted else None}

    # ── headline (PHASE1 section 3, with the cleanliness term from the bench) ──
    quality_ok = 1.0 if summary.get("quality", {}).get("errors", 1) == 0 else 0.0
    terms = [(0.5, res["dims"]["recall"]), (0.2, res["holes"]["recall"]),
             (0.15, res["views"]["coverage"]), (0.15, quality_ok)]
    weight = sum(w for w, v in terms if v is not None)
    res["headline"] = round(sum(w * v for w, v in terms if v is not None) / weight, 3) if weight else None
    res["notes_reference"] = ref.get("notes", [])
    return res


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    args = sys.argv[1:]

    def opt(name, default):
        return type(default)(args[args.index(name) + 1]) if name in args else default

    with open(args[0], encoding="utf-8") as f:
        summary = json.load(f)
    with open(args[1], encoding="utf-8") as f:
        ref = json.load(f)
    res = score(summary, ref, opt("--tol-rel", 0.005), opt("--tol-abs", 0.05))
    text = json.dumps(res, indent=1, ensure_ascii=False)
    if "--out" in args:
        with open(args[args.index("--out") + 1], "w", encoding="utf-8") as f:
            f.write(text + "\n")
    else:
        print(text)


if __name__ == "__main__":
    main()
