# FreeCAD/OCCT census: how many faces/edges of each STEP part fall in the defect classes
# diagnosed on parts 217 / 211 / 249.  One line per part.  Read-only on the STEP data.
# env: OCCT_ROOT=<folder with <id>/output.step>  OCCT_IDS="103,109,..."
import os, sys, math
import FreeCAD, Part

def planar(face):
    try:
        u0, u1, v0, v1 = face.ParameterRange
        n0 = None
        for a in (0.1, 0.5, 0.9):
            for b in (0.1, 0.5, 0.9):
                n = face.Surface.normal(u0 + (u1 - u0) * a, v0 + (v1 - v0) * b)
                if n0 is None: n0 = n
                elif (n - n0).Length > 1e-7 and (n + n0).Length > 1e-7: return False
        return True
    except Exception:
        return False

def normals_within(face, ang):
    # Roshera's Surface::is_planar: every sampled normal within `ang` of the first one
    try:
        u0, u1, v0, v1 = face.ParameterRange
        n0 = None
        for i in range(6):
            for j in range(6):
                n = face.Surface.normal(u0 + (u1 - u0) * i / 5.0, v0 + (v1 - v0) * j / 5.0)
                if n0 is None: n0 = n
                elif n.getAngle(n0) > ang: return False
        return True
    except Exception:
        return False

def loop_flat_within(face, tol):
    # the mesher's own test: outer-loop samples within tol of their best-fit (Newell) plane
    try:
        pts = face.OuterWire.discretize(Number=120)
        n = len(pts); nx = ny = nz = 0.0
        for i in range(n):
            a, b = pts[i], pts[(i + 1) % n]
            nx += (a.y - b.y) * (a.z + b.z); ny += (a.z - b.z) * (a.x + b.x); nz += (a.x - b.x) * (a.y + b.y)
        m = math.sqrt(nx * nx + ny * ny + nz * nz)
        if m < 1e-12: return False
        d0 = (nx * pts[0].x + ny * pts[0].y + nz * pts[0].z) / m
        return all(abs((nx * p.x + ny * p.y + nz * p.z) / m - d0) <= tol for p in pts)
    except Exception:
        return False

def cname(e):
    try: return e.Curve.__class__.__name__
    except Exception: return "?"

def census(path):
    shape = Part.read(path)
    c = dict(faces=len(shape.Faces), A_trim=0, B_planar_rev=0, B2_curved_meshed_flat=0, C_sphere_seam=0, D_cone_apex=0,
             E_closed_bspline=0, E2_annulus=0, F_rev_profile=0, H_thin_torus=0, I_pole_micro=0,
             T_edges_gap_gt_1e3=0)
    seen = set()
    for e in shape.Edges:
        h = e.hashCode()
        if h in seen: continue
        seen.add(h)
        try:
            cv = e.Curve
            if cv.__class__.__name__ not in ("BSplineCurve", "BezierCurve"): continue
            worst = 0.0
            for v in e.Vertexes:
                worst = max(worst, (cv.value(cv.parameter(v.Point)) - v.Point).Length)
            frac = abs(e.LastParameter - e.FirstParameter) / abs(cv.LastParameter - cv.FirstParameter)
            if 1e-3 < worst < 0.5 and frac < 0.99: c["A_trim"] += 1
        except Exception:
            pass
    for f in shape.Faces:
        s = f.Surface; name = s.__class__.__name__
        try:
            edges = f.OuterWire.Edges
        except Exception:
            edges = f.Edges
        def is_deg(e):
            try: return bool(e.Degenerated) or e.Length < 1e-2
            except Exception: return False
        seam = False
        for i, e in enumerate(f.Edges):
            for e2 in f.Edges[i + 1:]:
                if e.isSame(e2): seam = True
        try:
            # tolerant boundary: any edge noticeably off this face's surface
            for e in f.Edges:
                if is_deg(e): continue
                p = e.valueAt(0.5 * (e.FirstParameter + e.LastParameter))
                uv = s.parameter(p)
                if (s.value(uv[0], uv[1]) - p).Length > 1e-3:
                    c["T_edges_gap_gt_1e3"] += 1
        except Exception:
            pass
        if name not in ("Plane",) and f.Orientation == "Reversed" and name in ("SurfaceOfExtrusion", "SurfaceOfRevolution") and (normals_within(f, 0.1) or loop_flat_within(f, 1e-2)):
            c["B_planar_rev"] += 1
        if name in ("SurfaceOfExtrusion", "SurfaceOfRevolution") and normals_within(f, 0.1) and not loop_flat_within(f, 1e-2):
            c["B2_curved_meshed_flat"] += 1
        full_u = False; full_v = False
        try:
            fu0, fu1, fv0, fv1 = f.ParameterRange; su0, su1, sv0, sv1 = s.bounds()
            full_u = s.isUClosed() and abs(fu1 - fu0) >= 0.999 * abs(su1 - su0)
            full_v = s.isVClosed() and abs(fv1 - fv0) >= 0.999 * abs(sv1 - sv0)
        except Exception:
            pass
        if name == "Sphere":
            if full_u and len(f.Edges) > 3: c["C_sphere_seam"] += 1
            if any(is_deg(e) and e.Length > 1e-9 for e in f.Edges): c["I_pole_micro"] += 1
        if name == "Cone":
            has_apex = any(is_deg(e) for e in f.Edges)
            nonc = any((not is_deg(e)) and cname(e) not in ("Circle", "Line") for e in f.Edges)
            off = 0.0
            try:
                for e in f.Edges:
                    if is_deg(e): continue
                    for a in (0.25, 0.5, 0.75):
                        p = e.valueAt(e.FirstParameter + (e.LastParameter - e.FirstParameter) * a)
                        uv = s.parameter(p); off = max(off, (s.value(uv[0], uv[1]) - p).Length)
            except Exception:
                pass
            if has_apex and nonc and off > 2e-6: c["D_cone_apex"] += 1
        if name == "BSplineSurface" and (full_u or full_v):
            c["E_closed_bspline"] += 1
        if name == "Plane" and len(f.Wires) == 2:
            try:
                inner = [w for w in f.Wires if not w.isSame(f.OuterWire)][0]
                if len(inner.Edges) == 1 and cname(inner.Edges[0]) == "Circle" and len(f.OuterWire.Edges) > 1:
                    cen = inner.Edges[0].Curve.Center
                    pts = f.OuterWire.discretize(Number=200)
                    rs = [(p - cen).Length for p in pts]; m = sum(rs) / len(rs)
                    if max(abs(r - m) for r in rs) <= 0.02 * m: c["E2_annulus"] += 1
            except Exception:
                pass
        if name == "SurfaceOfRevolution":
            try:
                ax = s.Direction; loc = s.Location; cv = s.BasisCurve
                u0, u1, v0, v1 = f.ParameterRange
                az = []; rad = []; e1 = None
                for k in range(17):
                    p = cv.value(v0 + (v1 - v0) * k / 16) - loc
                    r = p - ax * p.dot(ax)
                    if r.Length < 1e-9: continue
                    if e1 is None: e1 = r * (1.0 / r.Length); e2 = ax.cross(e1)
                    az.append(math.atan2(r.dot(e2), r.dot(e1))); rad.append(r.Length)
                if az and (max(az) - min(az)) * max(rad) > 1e-4: c["F_rev_profile"] += 1
            except Exception:
                pass
        if name == "Toroid":
            try:
                u0, u1, v0, v1 = f.ParameterRange
                lu = abs(u1 - u0) * s.MajorRadius; lv = abs(v1 - v0) * s.MinorRadius
                if min(lu, lv) > 0 and max(lu, lv) / min(lu, lv) > 30: c["H_thin_torus"] += 1
            except Exception:
                pass
    return c

root = os.environ["OCCT_ROOT"]
for pid in os.environ["OCCT_IDS"].split(","):
    try:
        c = census(os.path.join(root, pid, "output.step"))
        print("part %s: " % pid + " ".join("%s=%d" % kv for kv in c.items()))
    except Exception as ex:
        print("part %s: FAILED %s" % (pid, ex))
    sys.stdout.flush()
