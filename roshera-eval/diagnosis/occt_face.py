# FreeCAD/OCCT: describe faces of a STEP part by index (OCCT face index == Roshera face id
# for these single-solid parts: both follow the STEP shell order).
# env: OCCT_STEP=<file>  OCCT_FACES="415,32"  [OCCT_SAMPLES=5]
import os, sys
import FreeCAD, Part
shape = Part.read(os.environ["OCCT_STEP"])
ns = int(os.environ.get("OCCT_SAMPLES", "5"))
faces = shape.Faces
print("faces:", len(faces))
for i in [int(x) for x in os.environ["OCCT_FACES"].split(",")]:
    f = faces[i]
    s = f.Surface
    name = s.__class__.__name__
    extra = ""
    for attr in ("Radius", "MajorRadius", "MinorRadius", "SemiAngle", "Axis", "Center", "Apex", "Position"):
        try:
            extra += " %s=%s" % (attr, getattr(s, attr))
        except Exception:
            pass
    for attr in ("isUPeriodic", "isVPeriodic", "isUClosed", "isVClosed"):
        try:
            extra += " %s=%s" % (attr, getattr(s, attr)())
        except Exception:
            pass
    try:
        extra += " surfbounds=%s" % (tuple(round(x, 5) for x in s.bounds()),)
    except Exception:
        pass
    print("face #%d %s area %.6f orientation %s valid %s tol %.2e wires %d faceUV %s%s" % (
        i, name, f.Area, f.Orientation, f.isValid(), f.Tolerance, len(f.Wires),
        tuple(round(x, 5) for x in f.ParameterRange), extra))
    for w in f.Wires:
        print("  wire: %d edges, outer %s" % (len(w.Edges), w.isSame(f.OuterWire)))
        for e in w.OrderedEdges:
            try:
                cn = e.Curve.__class__.__name__
            except Exception:
                cn = "degenerate?"
            deg = False
            try:
                deg = e.Degenerated
            except Exception:
                pass
            pts = []
            for k in range(ns):
                t = e.FirstParameter + (e.LastParameter - e.FirstParameter) * k / (ns - 1)
                try:
                    p = e.valueAt(t)
                    uv = s.parameter(p)
                    q = s.value(uv[0], uv[1])
                    pts.append("(%.4f,%.4f,%.4f|uv %.4f,%.4f|off %.1e)" % (p.x, p.y, p.z, uv[0], uv[1], (q - p).Length))
                except Exception as ex:
                    pts.append("?")
            nuse = sum(1 for e2 in w.Edges if e2.isSame(e))
            print("    %s len %.5f tol %.1e ori %s degenerated %s closed %s uses-in-wire %d\n       %s" % (
                cn, e.Length, e.Tolerance, e.Orientation, deg, e.isClosed(), nuse, " ".join(pts)))
sys.stdout.flush()
