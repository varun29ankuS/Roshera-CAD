# FreeCAD/OCCT: for SurfaceOfRevolution faces, does the profile (basis curve) lie in ONE
# meridian half-plane?  Reports the azimuth of the profile about the axis along the face's
# v-range; a spread means a closest-point that "rotates the query back onto the reference
# meridian" is wrong by spread * radius.
# env: OCCT_STEP, [OCCT_FACES="199,..."] (default: every SurfaceOfRevolution face)
import os, sys, math
import FreeCAD, Part
shape = Part.read(os.environ["OCCT_STEP"])
want = os.environ.get("OCCT_FACES")
ids = [int(x) for x in want.split(",")] if want else range(len(shape.Faces))
n_rev = 0; n_bad = 0
for i in ids:
    f = shape.Faces[i]; s = f.Surface
    if s.__class__.__name__ != "SurfaceOfRevolution": continue
    n_rev += 1
    ax = s.Direction; loc = s.Location; c = s.BasisCurve
    u0, u1, v0, v1 = f.ParameterRange
    ref = None; az = []; rad = []
    for k in range(33):
        t = v0 + (v1 - v0) * k / 32
        p = c.value(t) - loc
        h = p.dot(ax); r = p - ax * h
        if r.Length < 1e-9: continue
        if ref is None:
            e1 = r * (1.0 / r.Length); e2 = ax.cross(e1); ref = True
        az.append(math.atan2(r.dot(e2), r.dot(e1))); rad.append(r.Length)
    spread = (max(az) - min(az)) if az else 0.0
    miss = spread * (max(rad) if rad else 0.0)
    if miss > 1e-6: n_bad += 1
    if want or miss > 1e-6:
        print("face #%d SurfaceOfRevolution: basis %s, axis %s at %s; profile azimuth spread %.6f rad over the face's range; radius %.3f..%.3f; => meridian-assumption miss up to %.4f mm" % (
            i, c.__class__.__name__, tuple(round(x, 4) for x in ax), tuple(round(x, 4) for x in loc), spread, min(rad), max(rad), miss))
print("SurfaceOfRevolution faces: %d; with a profile off one meridian plane (miss > 1e-6): %d" % (n_rev, n_bad))
sys.stdout.flush()
