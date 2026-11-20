#!/usr/bin/env python3
"""Drawing benchmark: Roshera's automatic sheet, FreeCAD TechDraw's, and the
human reference, per part, side by side and measured.

    python -I bench.py --out runs/<label> [--only 101,105] [--set cadgen|nist|all]
                       [--skip-ours] [--skip-freecad] [--skip-images]

Inputs (override with the environment variables named):

  ROSHERA_BENCH_STEPS      CADGenBench reconstructions: <dir>/<id>/output.step
  ROSHERA_BENCH_REFERENCE  human reference drawings:    <dir>/<id>/input.png
  ROSHERA_BENCH_NIST       NIST MBE models:             <dir>/step/*.stp, <dir>/*.pdf
  ROSHERA_BENCH_EXE        the built `drawing_bench` example
  ROSHERA_FREECADCMD, ROSHERA_INKSCAPE, ROSHERA_MAGICK

Per part it writes `<out>/<id>/`:

  ours.svg / ours.pdf / ours.dxf / summary.json   (export-engine example `drawing_bench`)
  spaces.json / spaces.svg                        the space map and its boxes over the sheet
  freecad.svg / freecad.json                      (`freecad_sheet.py` under freecadcmd)
  ours.png, freecad.png, reference.png
  contact.png                                     reference | ours | FreeCAD
  score.json                                      when refs/<id>.json exists (score.py)

and `<out>/index.json` + `<out>/results.md` with the aggregate numbers.

Every stage has a time limit and every failure is recorded in the index:
a part that cannot be drawn is a result, not a crash.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


def env(name, default):
    return os.environ.get(name) or default


STEPS = env("ROSHERA_BENCH_STEPS",
            r"C:\Users\Varun Sharma\Downloads\standard-manufacturing-co_smc-harness-v1-2-fable-5-2-max_20261113-034752")
# The reference drawings and NIST models are third-party data kept beside this
# script, untracked (`.gitignore`; re-fetch per PHASE1.md).
REFERENCE = env("ROSHERA_BENCH_REFERENCE", os.path.join(HERE, "reference"))
NIST = env("ROSHERA_BENCH_NIST", os.path.join(HERE, "reference-nist"))
EXE = env("ROSHERA_BENCH_EXE",
          os.path.join(REPO, "roshera-backend", "target", "debug", "examples", "drawing_bench.exe"))
FREECADCMD = env("ROSHERA_FREECADCMD", r"C:\Program Files\FreeCAD 1.0\bin\freecadcmd.exe")
INKSCAPE = env("ROSHERA_INKSCAPE", r"C:\Program Files\Inkscape\bin\inkscape.exe")
MAGICK = env("ROSHERA_MAGICK", r"C:\Program Files\ImageMagick-7.1.2-Q16-HDRI\magick.exe")

# A1's bar is 30 s per sheet; a part still running at 300 s is reported as
# timed out at its last recorded stage, which is as honest as 600 s and keeps
# one part inside a 10-minute tool window together with FreeCAD and images.
OURS_LIMIT_S = int(env("ROSHERA_BENCH_OURS_LIMIT_S", "300"))
FREECAD_LIMIT_S = 240
IMAGE_LIMIT_S = 180


def run(cmd, limit, extra_env=None):
    """-> (ok, seconds, tail of output). Never raises."""
    t = time.time()
    e = dict(os.environ)
    if extra_env:
        e.update(extra_env)
    try:
        p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=limit, env=e)
        return p.returncode == 0, time.time() - t, p.stdout.decode("utf-8", "replace")[-400:]
    except subprocess.TimeoutExpired:
        return False, time.time() - t, "TIMEOUT after %d s" % limit
    except OSError as err:
        return False, time.time() - t, "could not start: %s" % err


def load(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def parts(which):
    out = []
    if which in ("cadgen", "all") and os.path.isdir(REFERENCE):
        for pid in sorted(os.listdir(REFERENCE)):
            ref = os.path.join(REFERENCE, pid, "input.png")
            step = os.path.join(STEPS, pid, "output.step")
            if os.path.isfile(ref) and os.path.isfile(step):
                out.append({"id": pid, "set": "cadgen", "step": step, "reference": ref})
    if which in ("nist", "all") and os.path.isdir(os.path.join(NIST, "step")):
        pdfs = [f for f in os.listdir(NIST) if f.lower().endswith(".pdf")]
        for f in sorted(os.listdir(os.path.join(NIST, "step"))):
            if not f.lower().endswith((".stp", ".step")):
                continue
            # nist_ctc_01_asme1_rd.stp -> ctc_01
            bits = f.split("_")
            pid = "%s_%s" % (bits[1], bits[2]) if len(bits) > 2 else os.path.splitext(f)[0]
            key = pid.replace("_", "").lower()
            pdf = next((p for p in pdfs if key in p.replace("_", "").replace("-", "").lower()), None)
            out.append({"id": "nist_" + pid, "set": "nist", "step": os.path.join(NIST, "step", f),
                        "reference": os.path.join(NIST, pdf) if pdf else None})
    return out


def to_png(src, dst, width):
    if not src or not os.path.isfile(src):
        return False
    if src.lower().endswith(".png"):
        ok, _, _ = run([MAGICK, src, "-resize", "%dx" % width, dst], IMAGE_LIMIT_S)
        return ok
    ok, _, _ = run([INKSCAPE, src, "--export-type=png", "--export-width=%d" % width,
                    "--export-background=#ffffff", "--export-filename=" + dst], IMAGE_LIMIT_S)
    return ok and os.path.isfile(dst)


def contact(folder, pid, caption):
    """reference | ours | FreeCAD, each 1500 px high, with a caption strip."""
    tiles = []
    for name, label in (("reference.png", "HUMAN REFERENCE"), ("ours.png", "ROSHERA (automatic)"),
                        ("freecad.png", "FREECAD 1.0 TECHDRAW (automatic, views only)")):
        path = os.path.join(folder, name)
        if os.path.isfile(path):
            tiles += ["(", path, "-resize", "x1500", "-background", "white", "-gravity", "north",
                      "-splice", "0x60", "-pointsize", "34", "-fill", "black",
                      "-annotate", "+0+12", label, ")"]
        else:
            tiles += ["(", "-size", "1100x1560", "xc:white", "-gravity", "center", "-pointsize", "40",
                      "-fill", "#a00000", "-annotate", "+0+0", label + "\nNOT PRODUCED", ")"]
    out = os.path.join(folder, "contact.png")
    ok, _, tail = run([MAGICK] + tiles + ["+append", "-background", "white", "-gravity", "south",
                                          "-splice", "0x70", "-pointsize", "30", "-fill", "black",
                                          "-annotate", "+0+18", "%s   %s" % (pid, caption), out], IMAGE_LIMIT_S)
    return ok


def caption_of(s):
    if not s or s.get("stage") != "done":
        return "ours: FAILED at %s" % (s or {}).get("stage", "start")
    c = s.get("certificate", {})
    counts = c.get("counts", {})
    return ("ours: %s %s, %d inked, %d not dimensioned, layout errors %d, %d ms"
            % ("SOUND" if c.get("sound") else "NOT SOUND",
               "COMPLETE" if c.get("complete") else "INCOMPLETE",
               s["sheet"].get("dimensions_inked", 0), counts.get("not_dimensioned", 0),
               s.get("quality", {}).get("errors", 0), s["sheet"].get("ms", 0)))


def pct(values, q):
    if not values:
        return None
    v = sorted(values)
    return v[min(len(v) - 1, int(round(q * (len(v) - 1))))]


def aggregate(rows):
    done = [r for r in rows if r.get("ours", {}).get("stage") == "done"]
    cert = [r["ours"]["certificate"] for r in done if "counts" in r["ours"].get("certificate", {})]
    sheet_ms = [r["ours"]["sheet"]["ms"] for r in done]
    nd = [c["counts"].get("not_dimensioned", 0) for c in cert]
    reasons = {}
    for c in cert:
        for k, v in c.get("omissions_by_reason", {}).items():
            reasons[k] = reasons.get(k, 0) + v
    kinds = {}
    for r in done:
        for k, (e, w) in r["ours"].get("quality", {}).get("by_kind", {}).items():
            a = kinds.setdefault(k, [0, 0, 0])
            a[0] += e
            a[1] += w
            a[2] += 1 if e else 0
    fc = [r["freecad"] for r in rows if r.get("freecad")]
    fc_ok = [f for f in fc if f.get("ok")]
    return {
        "parts": len(rows),
        "ours_built": len(done),
        "ours_failed": [{"id": r["id"], "stage": r.get("ours", {}).get("stage"),
                         "why": r.get("ours_tail") or r.get("ours", {}).get("error")
                         or r.get("ours", {}).get("sheet", {}).get("error")
                         or r.get("ours", {}).get("import", {}).get("error")}
                        for r in rows if r.get("ours", {}).get("stage") != "done"],
        "import_sound": sum(1 for r in done if r["ours"].get("solid", {}).get("sound")),
        "sheet_ms_median": pct(sheet_ms, 0.5),
        "sheet_ms_p95": pct(sheet_ms, 0.95),
        "sheet_ms_max": max(sheet_ms) if sheet_ms else None,
        "certificate_sound": sum(1 for c in cert if c.get("sound")),
        "certificate_complete": sum(1 for c in cert if c.get("complete")),
        "not_dimensioned_total": sum(nd),
        "not_dimensioned_median": pct(nd, 0.5),
        "not_dimensioned_max": max(nd) if nd else None,
        "omissions_by_reason": reasons,
        "stale_or_dangling_sheets": sum(1 for c in cert
                                        if c["counts"].get("stale", 0) + c["counts"].get("dangling", 0)),
        "omitted_bore_sheets": sum(1 for c in cert if c["counts"].get("omitted", 0)),
        "layout_clean_sheets": sum(1 for r in done if r["ours"].get("quality", {}).get("errors", 1) == 0),
        "layout_errors_total": sum(r["ours"].get("quality", {}).get("errors", 0) for r in done),
        # kind -> [errors, warnings, sheets with an error of that kind]
        "layout_issues_by_kind": kinds,
        "inked_median": pct([r["ours"]["sheet"].get("dimensions_inked", 0) for r in done], 0.5),
        "with_section": sum(1 for r in done if r["ours"]["sheet"].get("has_section")),
        "with_hole_table": sum(1 for r in done if r["ours"]["sheet"].get("hole_table")),
        "exports_ok": {k: sum(1 for r in done if r["ours"].get("renders", {}).get(k, {}).get("ok"))
                       for k in ("svg", "pdf", "dxf")},
        "freecad_built": len(fc_ok),
        "freecad_ms_median": pct([f.get("sheet_ms", 0) for f in fc_ok], 0.5),
        "freecad_failed": [{"id": r["id"], "why": (r.get("freecad") or {}).get("error") or r.get("freecad_tail")}
                           for r in rows if r.get("freecad") is not None and not r["freecad"].get("ok")],
        "scored": [r["score"] for r in rows if r.get("score")],
    }


def top_rules(by_rule, n=3):
    """The n rules with the most findings, errors first: 'TEXT-HEIGHT 1E/21W'."""
    ranked = sorted(by_rule.items(), key=lambda kv: (-kv[1][0], -(kv[1][0] + kv[1][1]), kv[0]))
    return ", ".join("%s %dE/%dW" % (k, v[0], v[1]) for k, v in ranked[:n])


def results_md(agg, rows, out):
    lines = ["| id | set | ours | faces | sheet ms | inked | not dim. | layout err | sound | complete "
             "| grade | below bar | top failing rules | FreeCAD | FC ms |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in rows:
        s = r.get("ours") or {}
        c = s.get("certificate", {})
        f = r.get("freecad") or {}
        sp = s.get("spaces") or {}
        if s.get("stage") == "done":
            lines.append("| %s | %s | ok | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (
                r["id"], r["set"], s.get("solid", {}).get("faces"), s["sheet"].get("ms"),
                s["sheet"].get("dimensions_inked"), c.get("counts", {}).get("not_dimensioned"),
                s.get("quality", {}).get("errors"), "yes" if c.get("sound") else "NO",
                "yes" if c.get("complete") else "no",
                "%.3f" % sp["grade"] if sp.get("grade") is not None else "",
                "%s / %s" % (sp.get("below_bar"), sp.get("spaces")) if sp else "",
                top_rules(sp.get("by_rule") or {}),
                "ok" if f.get("ok") else "FAILED", f.get("sheet_ms", "")))
        else:
            lines.append("| %s | %s | FAILED at %s | | | | | | | | | | | %s | %s |" % (
                r["id"], r["set"], s.get("stage", "start"), "ok" if f.get("ok") else "FAILED", f.get("sheet_ms", "")))
    with open(os.path.join(out, "results.md"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")


CAP_HEIGHT_OF_EM = 0.70  # capital height of a sans-serif face per em


def svg_checks(path):
    """A6 facts read from the rendered SVG: the size of every text element
    (inline style, else its class rule) as capital height, and trailing zeros
    in dimension text. None when the SVG is missing."""
    try:
        with open(path, encoding="utf-8") as f:
            svg = f.read()
    except OSError:
        return None
    style = re.search(r"<style>(.*?)</style>", svg, re.S)
    rules = {}
    for sel, body in re.findall(r"\.([\w-]+)\s*\{([^}]*)\}", style.group(1) if style else ""):
        m = re.search(r"font(?:-size)?\s*:[^;]*?([\d.]+)px", body)
        if m:
            rules[sel] = float(m.group(1))
    by_class = {}
    dim_text = []
    for attrs, body in re.findall(r"<text([^>]*)>(.*?)</text>", svg, re.S):
        m = re.search(r'class="([^"]*)"', attrs)
        cls = m.group(1).split() if m else []
        inline = re.search(r"font-size\s*:\s*([\d.]+)px", attrs)
        size = float(inline.group(1)) if inline else next((rules[c] for c in cls if c in rules), None)
        if size is None:
            continue
        by_class.setdefault(cls[0] if cls else "(none)", []).append(size)
        if "dim-text" in cls:
            dim_text.append(re.sub(r"<[^>]+>", "", body))
    caps = {k: round(min(v) * CAP_HEIGHT_OF_EM, 2) for k, v in by_class.items()
            if k not in ("watermark",)}
    return {
        "cap_height_min_by_class": caps,
        "dim_cap_height_mm": caps.get("dim-text"),
        "text_classes_under_2_5": sorted(k for k, v in caps.items() if v < 2.5),
        "dim_texts": len(dim_text),
        "dim_texts_with_trailing_zero": sum(1 for t in dim_text if re.search(r"\d\.\d*0(?!\d)", t)),
    }


ORTHO = ("FRONT", "TOP", "RIGHT", "LEFT", "BOTTOM", "REAR")


def a_checks(row, folder):
    """Reference-free per-sheet checks (critique section 5.1) from the
    harness's own records. Every value is measured, none judged."""
    s = row.get("ours") or {}
    if s.get("stage") != "done":
        return {"built": False, "stage": s.get("stage"), "timed_out": bool(s.get("timed_out_at_stage"))}
    c = s.get("certificate", {})
    counts = c.get("counts", {})
    q = s.get("quality", {})
    by = q.get("by_kind", {})
    views = s["sheet"].get("views", [])
    stated = counts.get("consistent", 0)
    nd = counts.get("not_dimensioned", 0)
    mis = by.get("ProjectionMisaligned", [0, 0])
    return {
        "built": True,
        "a1_sheet_ms": s["sheet"].get("ms"),
        "a2_sound": bool(c.get("sound")),
        "a2_stale_or_dangling": counts.get("stale", 0) + counts.get("dangling", 0),
        "a2_omitted_bores": counts.get("omitted", 0),
        "a3_complete": bool(c.get("complete")),
        "a3_not_dimensioned": nd,
        # facts stated consistently / (those + model dimensions not stated)
        "a3_coverage": round(stated / (stated + nd), 3) if stated + nd else None,
        "a4_redundant": by.get("RedundantDimension", [0, 0])[0],
        "a5_label_collisions": by.get("DimensionLabelCollision", [0, 0])[0],
        "a5_layout_errors": q.get("errors", 0),
        "a7_views": len(views),
        "a7_ortho_views": sum(1 for v in views if v["name"] in ORTHO),
        "a7_sheet": s["sheet"].get("size"),
        "a7_utilization": q.get("sheet_utilization"),
        "a7_misaligned": mis[0] + mis[1],
        "a8_tilt_deg": (s.get("orientation") or {}).get("tilt_deg"),
        # Before framing (absent on sheets from before the bench measured it,
        # where the sheet was drawn as imported: the drawn tilt IS this).
        "a8_tilt_as_imported_deg": (s.get("orientation_as_imported") or s.get("orientation") or {}).get("tilt_deg"),
        "frame": s.get("frame"),
        "section": bool(s["sheet"].get("has_section")),
        "section_dims": sum(v.get("dimensions", 0) for v in views if v["name"].startswith("SECTION")),
        "a10_exports": all(s.get("renders", {}).get(k, {}).get("ok") for k in ("svg", "pdf", "dxf")),
        "svg": svg_checks(os.path.join(folder, "ours.svg")),
        # C: drafting craft from the space map (absent on sheets from before
        # the map): the grade is the worst space, never a mean.
        "c_grade": (s.get("spaces") or {}).get("grade"),
        "c_below_bar": (s.get("spaces") or {}).get("below_bar"),
        "c_bar": (s.get("spaces") or {}).get("bar"),
        "c_spaces": (s.get("spaces") or {}).get("spaces"),
        "c_worst": (s.get("spaces") or {}).get("worst"),
        "c_by_rule": (s.get("spaces") or {}).get("by_rule") or {},
    }


def rows_on_disk(out):
    """Every part folder under `out` with a row.json: chunked runs compose,
    because the aggregate is always over what is on disk."""
    rows = []
    for name in sorted(os.listdir(out)):
        r = load(os.path.join(out, name, "row.json"))
        if r:
            rows.append(r)
    return rows


def mean(v):
    return round(sum(v) / len(v), 3) if v else None


def summarize_checks(rows):
    built = [r["checks"] for r in rows if (r.get("checks") or {}).get("built")]
    out = {"parts": len(rows), "built": len(built),
           "timed_out": sum(1 for r in rows if (r.get("checks") or {}).get("timed_out")),
           "failed_at": sorted("%s:%s" % (r["id"], r["checks"].get("stage")) for r in rows
                               if not (r.get("checks") or {}).get("built"))}
    if not built:
        return out
    ms = [b["a1_sheet_ms"] for b in built if b["a1_sheet_ms"] is not None]
    cov = [b["a3_coverage"] for b in built if b["a3_coverage"] is not None]
    tilt = [b["a8_tilt_deg"] for b in built if b["a8_tilt_deg"] is not None]
    svgs = [b["svg"] for b in built if b.get("svg")]
    out.update({
        "a1_ms_median": pct(ms, 0.5), "a1_ms_p95": pct(ms, 0.95),
        "a1_under_30s": sum(1 for m in ms if m < 30000),
        "a2_sound": sum(1 for b in built if b["a2_sound"]),
        "a2_sheets_with_stale_or_dangling": sum(1 for b in built if b["a2_stale_or_dangling"]),
        "a2_sheets_omitting_a_bore": sum(1 for b in built if b["a2_omitted_bores"]),
        "a3_complete": sum(1 for b in built if b["a3_complete"]),
        "a3_coverage_median": pct(cov, 0.5),
        "a3_coverage_min": min(cov) if cov else None,
        "a3_coverage_at_least_0_9": sum(1 for c in cov if c >= 0.9),
        "a4_sheets_with_redundant": sum(1 for b in built if b["a4_redundant"]),
        "a5_layout_clean": sum(1 for b in built if b["a5_layout_errors"] == 0),
        "a5_label_collisions_total": sum(b["a5_label_collisions"] for b in built),
        "a6_dim_cap_height_mm": sorted({t["dim_cap_height_mm"] for t in svgs if t["dim_cap_height_mm"]}),
        "a6_sheets_with_text_under_2_5": sum(1 for t in svgs if t["text_classes_under_2_5"]),
        "a6_text_classes_under_2_5": sorted({c for t in svgs for c in t["text_classes_under_2_5"]}),
        "a6_dim_texts": sum(t["dim_texts"] for t in svgs),
        "a6_dim_texts_with_trailing_zero": sum(t["dim_texts_with_trailing_zero"] for t in svgs),
        "a7_views_hist": {str(k): sum(1 for b in built if b["a7_views"] == k)
                          for k in sorted({b["a7_views"] for b in built})},
        "a7_ortho_views_hist": {str(k): sum(1 for b in built if b["a7_ortho_views"] == k)
                                for k in sorted({b["a7_ortho_views"] for b in built})},
        "a7_sheets_with_misaligned_views": sum(1 for b in built if b["a7_misaligned"]),
        "a8_measured": len(tilt),
        "a8_square_within_0_1deg": sum(1 for t in tilt if t <= 0.1),
        "a8_tilt_max_deg": round(max(tilt), 3) if tilt else None,
        "a8_as_imported_off_square_over_0_1deg": sorted(
            "%s:%.2f" % (r["id"], r["checks"]["a8_tilt_as_imported_deg"]) for r in rows
            if (r.get("checks") or {}).get("built") and (r["checks"].get("a8_tilt_as_imported_deg") or 0) > 0.1),
        "frames": {f: sum(1 for b in built if (b.get("frame") or "").startswith(f))
                   for f in ("AsModelled", "AsDesigned", "Squared", "AsPlaced")},
        "sections": sum(1 for b in built if b["section"]),
        "section_dims_total": sum(b["section_dims"] for b in built),
        "a10_all_exports": sum(1 for b in built if b["a10_exports"]),
    })
    graded = [b for b in built if b.get("c_grade") is not None]
    if graded:
        rules = {}
        for b in graded:
            for k, (e, w) in b["c_by_rule"].items():
                a = rules.setdefault(k, [0, 0, 0])
                a[0] += e
                a[1] += w
                a[2] += 1
        grades = [b["c_grade"] for b in graded]
        # the bar each sheet was scored against (drawing::spaces::SPACE_BAR)
        bar = next((b["c_bar"] for b in graded if b.get("c_bar") is not None), 0.8)
        out.update({
            "c_graded": len(graded),
            # the worst sheet's grade and the median sheet's: per-sheet grades
            # are worst-space minima, these only order the sheets
            "c_grade_min": round(min(grades), 3),
            "c_grade_median": round(pct(grades, 0.5), 3),
            "c_sheets_at_or_above_bar": sum(1 for g in grades if g >= bar),
            "c_below_bar_total": sum(b["c_below_bar"] or 0 for b in graded),
            "c_below_bar_median": pct([b["c_below_bar"] or 0 for b in graded], 0.5),
            # rule -> [errors, warnings, sheets with a finding of that rule]
            "c_findings_by_rule": dict(sorted(rules.items())),
        })
    return out


def summarize_scores(rows):
    sc = [r["score"] for r in rows if r.get("score")]
    if not sc:
        return None
    rec = [s["dims"]["recall"] for s in sc if s["dims"]["recall"] is not None]
    pre = [s["dims"]["precision"] for s in sc if s["dims"]["precision"] is not None]
    hol = [s["holes"]["recall"] for s in sc if s["holes"]["recall"] is not None]
    vw = [s["views"]["coverage"] for s in sc if s["views"]["coverage"] is not None]
    hd = [s["headline"] for s in sc if s["headline"] is not None]
    return {"parts": len(sc), "ids": [s["id"] for s in sc],
            "b1_recall_mean": mean(rec), "b1_recall_min": min(rec) if rec else None,
            "b2_precision_mean": mean(pre), "b3_hole_recall_mean": mean(hol),
            "view_coverage_mean": mean(vw), "headline_mean": mean(hd)}


def write_aggregate(out):
    rows = rows_on_disk(out)
    held = set((load(os.path.join(HERE, "split.json")) or {}).get("held_out", []))
    groups = {"all": rows,
              "cadgen": [r for r in rows if r["set"] == "cadgen"],
              "nist": [r for r in rows if r["set"] == "nist"],
              "cadgen_dev": [r for r in rows if r["set"] == "cadgen" and r["id"] not in held],
              "cadgen_held_out": [r for r in rows if r["set"] == "cadgen" and r["id"] in held]}
    agg = {}
    for name, sub in groups.items():
        if sub:
            agg[name] = {"checks": summarize_checks(sub), "scores": summarize_scores(sub),
                         "legacy": {k: v for k, v in aggregate(sub).items() if k != "scored"}}
    with open(os.path.join(out, "index.json"), "w", encoding="utf-8") as fh:
        json.dump({"aggregate": agg, "rows": rows}, fh, indent=1)
    results_md(agg, rows, out)
    return agg


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--set", default="all", choices=["cadgen", "nist", "all"])
    ap.add_argument("--only", default="")
    ap.add_argument("--skip-ours", action="store_true")
    ap.add_argument("--skip-freecad", action="store_true")
    ap.add_argument("--skip-images", action="store_true")
    ap.add_argument("--aggregate-only", action="store_true",
                    help="re-check and re-score the part folders on disk, run nothing, aggregate")
    ap.add_argument("--freecad-from", default="",
                    help="copy freecad.* and reference.png from this earlier run instead of re-running FreeCAD")
    ap.add_argument("--images-only", action="store_true",
                    help="redo PNGs and contact sheets of the part folders on disk, run nothing else")
    a = ap.parse_args()
    out = os.path.abspath(a.out)
    os.makedirs(out, exist_ok=True)
    todo = parts(a.set)
    if a.only:
        want = set(a.only.split(","))
        todo = [p for p in todo if p["id"] in want]
    if not todo:
        sys.exit("no parts found: check ROSHERA_BENCH_STEPS / ROSHERA_BENCH_REFERENCE / ROSHERA_BENCH_NIST")
    for i, p in enumerate(todo):
        folder = os.path.join(out, p["id"])
        if a.aggregate_only and not os.path.isdir(folder):
            continue
        os.makedirs(folder, exist_ok=True)
        row = load(os.path.join(folder, "row.json")) or {"id": p["id"], "set": p["set"]}
        if a.freecad_from:
            src = os.path.join(os.path.abspath(a.freecad_from), p["id"])
            for name in ("freecad.json", "freecad.svg", "freecad.png", "reference.png"):
                if os.path.isfile(os.path.join(src, name)) and not os.path.isfile(os.path.join(folder, name)):
                    shutil.copyfile(os.path.join(src, name), os.path.join(folder, name))
        if a.images_only:
            if os.path.isfile(os.path.join(folder, "ours.svg")):
                to_png(os.path.join(folder, "ours.svg"), os.path.join(folder, "ours.png"), 2400)
            contact(folder, p["id"], caption_of(row.get("ours") or {}))
            continue
        if not a.skip_ours and not a.aggregate_only:
            for stale in ("summary.json", "ours.svg", "ours.pdf", "ours.dxf", "ours.png"):
                if os.path.isfile(os.path.join(folder, stale)):
                    os.remove(os.path.join(folder, stale))
            ok, secs, tail = run([EXE, p["id"], p["step"], out], OURS_LIMIT_S)
            row["ours_seconds"] = round(secs, 1)
            row["ours_exit_ok"] = ok
            row["ours_tail"] = None if ok else tail
        row["ours"] = load(os.path.join(folder, "summary.json")) or {"stage": "start"}
        if "TIMEOUT" in (row.get("ours_tail") or ""):
            # The example writes its stage to disk on entering it, so this is
            # where the run was when the limit killed it.
            row["ours"]["timed_out_at_stage"] = row["ours"].get("stage")
        if not a.skip_freecad and not a.aggregate_only:
            for stale in ("freecad.json", "freecad.svg", "freecad.png"):
                if os.path.isfile(os.path.join(folder, stale)):
                    os.remove(os.path.join(folder, stale))
            ok, secs, tail = run([FREECADCMD, os.path.join(HERE, "freecad_sheet.py")], FREECAD_LIMIT_S,
                                 {"ROSHERA_BENCH_STEP": p["step"], "ROSHERA_BENCH_OUT": folder})
            row["freecad"] = load(os.path.join(folder, "freecad.json")) or {"ok": False}
            if not row["freecad"].get("ok") and "error" not in row["freecad"]:
                row["freecad_tail"] = tail
        else:
            row["freecad"] = load(os.path.join(folder, "freecad.json"))
        if not a.skip_images and not a.aggregate_only:
            to_png(os.path.join(folder, "ours.svg"), os.path.join(folder, "ours.png"), 2400)
            if not os.path.isfile(os.path.join(folder, "freecad.png")):
                to_png(os.path.join(folder, "freecad.svg"), os.path.join(folder, "freecad.png"), 2400)
            if not os.path.isfile(os.path.join(folder, "reference.png")):
                to_png(p["reference"], os.path.join(folder, "reference.png"), 2400)
            row["contact"] = contact(folder, p["id"], caption_of(row["ours"]))
        row["checks"] = a_checks(row, folder)
        row["score"] = None
        ref = os.path.join(HERE, "refs", p["id"] + ".json")
        if os.path.isfile(ref) and os.path.isfile(os.path.join(folder, "summary.json")):
            run([sys.executable, "-I", os.path.join(HERE, "score.py"),
                 os.path.join(folder, "summary.json"), ref,
                 "--out", os.path.join(folder, "score.json")], 60)
            row["score"] = load(os.path.join(folder, "score.json"))
        with open(os.path.join(folder, "row.json"), "w", encoding="utf-8") as fh:
            json.dump(row, fh, indent=1)
        print("[%d/%d] %s  %s" % (i + 1, len(todo), p["id"], caption_of(row["ours"])), flush=True)
    agg = write_aggregate(out)
    print(json.dumps({k: v["checks"] for k, v in agg.items()}, indent=1))


if __name__ == "__main__":
    main()
