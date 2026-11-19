"""FreeCAD TechDraw's automatic drawing of one STEP part, headless.

Run by `bench.py` as:

    freecadcmd.exe freecad_sheet.py

with the job in environment variables (freecadcmd treats extra command-line
arguments as files to open):

    ROSHERA_BENCH_STEP   the STEP file
    ROSHERA_BENCH_OUT    directory to write into

Writes `freecad.svg` (the sheet) and `freecad.json` (what was produced and
how long it took, or why it could not be).

What "automatic" means in FreeCAD 1.0: TechDraw places a projection group
(Front / Top / Right, third angle) and an isometric view with hidden lines.
It has NO automatic dimensioning: every dimension on a TechDraw sheet is
placed by hand. So this sheet carries views only, and the comparison with
it is a comparison of projection, hidden-line and layout quality.

Headless limits found by running it: the page cannot be exported with
`TechDrawGui.exportPageAsSvg` (no GUI module in freecadcmd), so the sheet is
assembled here from each view's own SVG (`TechDraw.viewPartAsSvg`) placed at
the view's position and scale on the page.
"""

import json
import os
import sys
import time

import FreeCAD
import Part

out_dir = os.environ.get("ROSHERA_BENCH_OUT", "")
step = os.environ.get("ROSHERA_BENCH_STEP", "")
result = {"step": step, "ok": False, "tool": "FreeCAD %s TechDraw" % ".".join(FreeCAD.Version()[:3])}


def finish():
    with open(os.path.join(out_dir, "freecad.json"), "w", encoding="utf-8") as f:
        json.dump(result, f, indent=1)
    # freecadcmd keeps running an interactive console otherwise.
    os._exit(0)


try:
    import TechDraw

    t0 = time.time()
    shape = Part.Shape()
    shape.read(step)
    result["import_ms"] = int((time.time() - t0) * 1000)
    solids = sorted(shape.Solids, key=lambda s: len(s.Faces), reverse=True)
    if not solids:
        result["error"] = "no solid in the file"
        finish()
    solid = solids[0]
    result["faces"] = len(solid.Faces)

    doc = FreeCAD.newDocument("bench")
    part = doc.addObject("Part::Feature", "Part")
    part.Shape = solid

    box = solid.BoundBox
    extent = max(box.XLength, box.YLength, box.ZLength, 1e-6)
    # Sheets and scales as a drafter picks them: the smallest ISO sheet on
    # which three views and an isometric fit at a preferred scale.
    sheets = [("A4", 297.0, 210.0), ("A3", 420.0, 297.0), ("A2", 594.0, 420.0),
              ("A1", 841.0, 594.0), ("A0", 1189.0, 841.0)]
    scales = [10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1, 0.05, 0.02]
    chosen = None
    for scale in scales:
        if scale > 1.0 and extent * scale > 120.0:
            continue
        for name, w, h in sheets:
            # Front + Right side by side, the isometric beside them; Front
            # under Top; margins and gaps between views.
            need_w = (box.XLength + box.YLength) * scale + 0.9 * extent * scale + 110.0
            need_h = (box.ZLength + box.YLength) * scale + 90.0
            if need_w <= w and need_h <= h:
                chosen = (name, w, h, scale)
                break
        if chosen:
            break
    if not chosen:
        chosen = ("A0", 1189.0, 841.0, 0.02)
    sheet_name, sheet_w, sheet_h, scale = chosen
    result.update({"sheet": sheet_name, "scale": scale})

    t1 = time.time()
    page = doc.addObject("TechDraw::DrawPage", "Page")
    # FreeCAD's own blank ISO sheet of that size.
    template = doc.addObject("TechDraw::DrawSVGTemplate", "Template")
    template.Template = os.path.join(
        FreeCAD.getResourceDir(), "Mod", "TechDraw", "Templates", "%s_Landscape_blank.svg" % sheet_name
    )
    page.Template = template
    page.Scale = scale

    group = doc.addObject("TechDraw::DrawProjGroup", "ProjGroup")
    page.addView(group)
    group.Source = [part]
    group.ProjectionType = "Third Angle"
    group.ScaleType = "Custom"
    group.Scale = scale
    front = group.addProjection("Front")
    group.Anchor.Direction = FreeCAD.Vector(0, -1, 0)
    group.Anchor.XDirection = FreeCAD.Vector(1, 0, 0)
    group.addProjection("Top")
    group.addProjection("Right")
    group.X = sheet_w * 0.36
    group.Y = sheet_h * 0.50

    iso = doc.addObject("TechDraw::DrawViewPart", "Iso")
    page.addView(iso)
    iso.Source = [part]
    iso.Direction = FreeCAD.Vector(1, -1, 1)
    iso.ScaleType = "Custom"
    iso.Scale = scale
    iso.X = sheet_w * 0.78
    iso.Y = sheet_h * 0.74

    views = [v for v in group.Views] + [iso]
    for v in views:
        v.HardHidden = True
        v.CoarseView = False
    doc.recompute()
    # Hidden-line removal runs on worker threads and hands its result back
    # through Qt signals: without a GUI nothing pumps them, so pump here.
    from PySide import QtCore

    app = QtCore.QCoreApplication.instance() or QtCore.QCoreApplication([])
    deadline = time.time() + 120.0
    while time.time() < deadline:
        app.processEvents()
        if all(len(v.getVisibleEdges()) > 0 for v in views):
            break
        time.sleep(0.1)
    result["hlr_finished"] = all(len(v.getVisibleEdges()) > 0 for v in views)
    result["sheet_ms"] = int((time.time() - t1) * 1000)

    pieces = []
    info = []
    for v in views:
        frag = TechDraw.viewPartAsSvg(v)
        # A projection group's children are positioned relative to the group.
        x = float(v.X)
        y = float(v.Y)
        if v in group.Views:
            x += float(group.X)
            y += float(group.Y)
        info.append({
            "name": v.Label,
            "visible_edges": len(v.getVisibleEdges()),
            "hidden_edges": len(v.getHiddenEdges()),
            "x": x, "y": y,
        })
        # TechDraw's page Y runs up; the SVG's runs down.
        # The fragment draws hidden edges as a thinner solid group; a
        # TechDraw page shows them dashed, so they are dashed here too.
        frag = frag.replace('stroke-width="0.35"', 'stroke-width="0.25" stroke-dasharray="2,1"')
        pieces.append('<g transform="translate(%.3f %.3f)">%s</g>' % (x, sheet_h - y, frag))
    result["views"] = info

    svg = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<svg xmlns="http://www.w3.org/2000/svg" width="%gmm" height="%gmm" viewBox="0 0 %g %g">'
        % (sheet_w, sheet_h, sheet_w, sheet_h),
        '<rect x="0" y="0" width="%g" height="%g" fill="#ffffff"/>' % (sheet_w, sheet_h),
        '<rect x="10" y="10" width="%g" height="%g" fill="none" stroke="#000" stroke-width="0.5"/>'
        % (sheet_w - 20, sheet_h - 20),
        '<text x="14" y="%g" font-family="sans-serif" font-size="4">%s  %s  scale %g  (views only: '
        'TechDraw has no automatic dimensions)</text>' % (sheet_h - 14, result["tool"], sheet_name, scale),
    ]
    svg.extend(pieces)
    svg.append("</svg>")
    with open(os.path.join(out_dir, "freecad.svg"), "w", encoding="utf-8") as f:
        f.write("\n".join(svg))
    result["ok"] = True
except Exception as e:  # reported, never swallowed: the bench lists it
    result["error"] = "%s: %s" % (type(e).__name__, e)

finish()
