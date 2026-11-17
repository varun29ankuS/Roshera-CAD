# FreeCAD/OCCT: for one face, walk the outer wire; project dense edge samples to the surface
# (closest point, clamped to the surface's own domain) and report, per edge, how far the
# samples sit OFF the surface, how many clamp onto a chart border, and whether the projected
# (u,v) walk doubles back (non-monotone) along that border.
# env: OCCT_STEP, OCCT_FACE, [OCCT_N=64]
import os, sys
import FreeCAD, Part
shape = Part.read(os.environ["OCCT_STEP"])
f = shape.Faces[int(os.environ["OCCT_FACE"])]
s = f.Surface
n = int(os.environ.get("OCCT_N", "64"))
u0, u1, v0, v1 = s.bounds()
print("surface", s.__class__.__name__, "bounds", (u0, u1, v0, v1))
eps = 1e-9
for k, e in enumerate(f.OuterWire.OrderedEdges):
    uv = []; off = 0.0
    for i in range(n + 1):
        t = e.FirstParameter + (e.LastParameter - e.FirstParameter) * i / n
        p = e.valueAt(t)
        a, b = s.parameter(p)
        a = min(max(a, u0), u1); b = min(max(b, v0), v1)
        off = max(off, (s.value(a, b) - p).Length)
        uv.append((a, b))
    on_border = sum(1 for a, b in uv if abs(a - u0) < eps or abs(a - u1) < eps or abs(b - v0) < eps or abs(b - v1) < eps)
    du = [uv[i + 1][0] - uv[i][0] for i in range(n)]; dv = [uv[i + 1][1] - uv[i][1] for i in range(n)]
    def flips(d):
        sg = [1 if x > 1e-12 else -1 if x < -1e-12 else 0 for x in d]
        nz = [x for x in sg if x]
        return sum(1 for i in range(len(nz) - 1) if nz[i] != nz[i + 1]), sum(1 for x in sg if x == 0)
    print("edge %d %s len %.4f tol %.1e: worst off-surface %.2e; %d/%d samples on a chart border; u reversals %s (reversals, stalls) v reversals %s; uv %s -> %s" % (
        k, e.Curve.__class__.__name__, e.Length, e.Tolerance, off, on_border, n + 1, flips(du), flips(dv),
        tuple(round(x, 6) for x in uv[0]), tuple(round(x, 6) for x in uv[-1])))
sys.stdout.flush()
