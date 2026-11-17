# FreeCAD/OCCT: for every edge of a STEP part, how far its two vertices sit from the edge's
# own 3D curve (the WHOLE curve, as an importer that must trim it would measure), the
# vertex tolerance OCCT stores, and what fraction of the whole curve the edge uses.
# env: OCCT_STEP, [OCCT_MIN=1e-4]
import os, sys
import FreeCAD, Part
shape = Part.read(os.environ["OCCT_STEP"])
lim = float(os.environ.get("OCCT_MIN", "1e-4"))
rows = []
seen = set()
faces_of = {}
for fi, f in enumerate(shape.Faces):
    for e in f.Edges:
        faces_of.setdefault(e.hashCode(), set()).add(fi)
for e in shape.Edges:
    h = e.hashCode()
    if h in seen: continue
    seen.add(h)
    try:
        c = e.Curve
    except Exception:
        continue
    name = c.__class__.__name__
    if name in ("Line",): continue
    worst = 0.0; vt = 0.0
    for v in e.Vertexes:
        try:
            t = c.parameter(v.Point)
            worst = max(worst, (c.value(t) - v.Point).Length)
        except Exception:
            pass
        vt = max(vt, v.Tolerance)
    try:
        frac = abs(e.LastParameter - e.FirstParameter) / abs(c.LastParameter - c.FirstParameter)
    except Exception:
        frac = float("nan")
    rows.append((worst, vt, e.Tolerance, name, e.Length, frac, sorted(faces_of.get(h, []))))
rows.sort(reverse=True)
print("edges (non-line):", len(rows))
for lo in (1e-6, 1e-5, 1e-4, 1e-3, 1e-2):
    print("  vertex farther than %.0e from its edge curve: %d edges; of those using < 99%% of the curve: %d" % (
        lo, sum(1 for r in rows if r[0] > lo), sum(1 for r in rows if r[0] > lo and r[5] < 0.99)))
for r in rows:
    if r[0] < lim: break
    print("  off %.3e vertex-tol %.1e edge-tol %.1e %s len %.4f uses %.3f of curve; faces %s" % r)
sys.stdout.flush()
