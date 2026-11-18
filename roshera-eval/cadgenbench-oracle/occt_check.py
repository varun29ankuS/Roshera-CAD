# Independent reference check with FreeCAD's OpenCASCADE: validity and the
# worst edge-to-face gap of a STEP part. Usage (env): OCCT_STEP=<file>
import os
import sys

import FreeCAD  # noqa: F401
import Part

path = os.environ["OCCT_STEP"]
shape = Part.read(path)
print("valid:", shape.isValid())
print("solids:", len(shape.Solids), "shells:", len(shape.Shells), "faces:", len(shape.Faces), "edges:", len(shape.Edges))
for s in shape.Solids:
    print("  solid volume %.4f closed %s" % (s.Volume, s.isClosed()))

# Edge -> faces using it.
users = {}
for fi, f in enumerate(shape.Faces):
    for e in f.Edges:
        users.setdefault(e.hashCode(), (e, []))[1].append((fi, f))

worst = []
for h, (e, faces) in users.items():
    if e.Length < 1e-9:
        continue
    gap = 0.0
    for k in range(9):
        p = e.valueAt(e.FirstParameter + (e.LastParameter - e.FirstParameter) * k / 8.0)
        for fi, f in faces:
            try:
                q = f.Surface.projectPoint(p, "NearestPoint")
                d = (q - p).Length
            except Exception:
                d = -1.0
            gap = max(gap, d)
    try:
        kind = e.Curve.__class__.__name__
    except Exception:
        kind = "?"
    worst.append((gap, e.Length, [fi for fi, _ in faces], kind, e.Tolerance))
worst.sort(reverse=True)
print("worst edge-to-surface gaps (gap, length, faces, curve, tolerance):")
for w in worst[:8]:
    print("  %.3e  len %.3f  faces %s  %s  tol %.1e" % w)
print("edges with gap > 1e-3:", sum(1 for w in worst if w[0] > 1e-3))
print("edges with gap > 1.0:", sum(1 for w in worst if w[0] > 1.0))
sys.stdout.flush()
