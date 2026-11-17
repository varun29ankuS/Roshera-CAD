# Reads a tess_one_face PROBE_DUMP of a sphere face; given centre + axis, lists facets whose
# three longitudes cover more than half a turn (what the certificate calls a boundary-
# crossing facet) with their real 3D size and distance to the pole.
# usage: sphere_polar_facets.py dump cx cy cz ax ay az radius
import sys, math
V = []; T = []
for line in open(sys.argv[1], errors="replace"):
    s = line.strip()
    if s.startswith("v ("): V.append(tuple(float(x) for x in s[3:-1].split(",")))
    elif s.startswith("t ["): T.append(tuple(int(x) for x in s[3:-1].split(",")))
c = tuple(float(x) for x in sys.argv[2:5]); a = tuple(float(x) for x in sys.argv[5:8]); R = float(sys.argv[8])
la = math.sqrt(sum(x * x for x in a)); a = tuple(x / la for x in a)
ref = (1, 0, 0) if abs(a[0]) < 0.9 else (0, 1, 0)
cr = lambda p, q: (p[1] * q[2] - p[2] * q[1], p[2] * q[0] - p[0] * q[2], p[0] * q[1] - p[1] * q[0])
e1 = cr(a, ref); l = math.sqrt(sum(x * x for x in e1)); e1 = tuple(x / l for x in e1); e2 = cr(a, e1)
dot = lambda p, q: sum(p[i] * q[i] for i in range(3))
def lon(p):
    w = tuple(p[i] - c[i] for i in range(3)); return math.atan2(dot(w, e2), dot(w, e1)) % (2 * math.pi)
poles = [tuple(c[i] + s * R * a[i] for i in range(3)) for s in (1, -1)]
n = 0
for t in T:
    us = sorted(lon(V[i]) for i in t)
    gaps = [us[1] - us[0], us[2] - us[1], us[0] + 2 * math.pi - us[2]]
    cov = 2 * math.pi - max(gaps)
    if cov >= math.pi - 1e-9:
        n += 1
        size = max(math.dist(V[t[k]], V[t[(k + 1) % 3]]) for k in range(3))
        dpole = [min(math.dist(V[i], p) for p in poles) for i in t]
        if n <= 6:
            print("  facet %s: u-coverage %.4f rad; longest 3D edge %.3e; vertex distances to the pole %s" % (
                t, cov, size, " ".join("%.2e" % d for d in dpole)))
print("verts %d tris %d; facets covering >= half a turn in u: %d" % (len(V), len(T), n))
