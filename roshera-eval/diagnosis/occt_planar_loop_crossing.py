# FreeCAD/OCCT re-enactment of Roshera's planar_face_self_intersects on one planar face:
# walk the outer wire in order, sample every edge from ITS OWN CURVE over [0,1) (no snap to
# the shared vertex), project to the plane, look for proper crossings of non-adjacent
# segments. Also reports the end-to-start jump between consecutive edges (the tolerant-vertex
# gap) and repeats the test with every edge end snapped to the shared vertex.
# env: OCCT_STEP, OCCT_FACE=<index>
import os, sys, math
import FreeCAD, Part
shape = Part.read(os.environ["OCCT_STEP"])
f = shape.Faces[int(os.environ["OCCT_FACE"])]
w = f.OuterWire
edges = w.OrderedEdges
def samples(e):
    pts = e.discretize(Deflection=1e-5)
    pts = [(p.x, p.y, p.z) for p in pts]
    return pts
chains = [samples(e) for e in edges]
# orient chains head-to-tail
def d(a, b): return math.dist(a, b)
for k in range(len(chains)):
    nxt = chains[(k + 1) % len(chains)]
    c = chains[k]
    if min(d(c[0], nxt[0]), d(c[0], nxt[-1])) < min(d(c[-1], nxt[0]), d(c[-1], nxt[-1])):
        chains[k] = c[::-1]
jumps = []
for k in range(len(chains)):
    nxt = chains[(k + 1) % len(chains)]
    jumps.append(min(d(chains[k][-1], nxt[0]), d(chains[k][-1], nxt[-1])))
print("edges", len(edges), "end-to-next-start jumps: max %.3e; all:" % max(jumps), " ".join("%.1e" % j for j in jumps))
print("edge tolerances:", " ".join("%.1e" % e.Tolerance for e in edges))
print("vertex tolerances max: %.3e" % max(v.Tolerance for v in w.Vertexes))
def test(chains, label):
    V = []; owner = []
    for k, c in enumerate(chains):
        for p in c[:-1]:
            V.append(p); owner.append(k)
    n = len(V)
    ax = f.Surface.Axis; N = (ax.x, ax.y, ax.z)
    ref = (1, 0, 0) if abs(N[0]) < 0.9 else (0, 1, 0)
    cr = lambda a, b: (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])
    U = cr(N, ref); lu = math.sqrt(sum(c * c for c in U)); U = tuple(c / lu for c in U); W = cr(N, U)
    P = [(sum(p[i] * U[i] for i in range(3)), sum(p[i] * W[i] for i in range(3))) for p in V]
    def orient(a, b, c): return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    # sweep by x-sorted buckets to stay fast
    segs = sorted(range(n), key=lambda i: min(P[i][0], P[(i + 1) % n][0]))
    hits = []
    active = []
    for i in segs:
        a1, a2 = P[i], P[(i + 1) % n]
        lo = min(a1[0], a2[0])
        active = [j for j in active if max(P[j][0], P[(j + 1) % n][0]) >= lo]
        for j in active:
            if abs(i - j) <= 1 or abs(i - j) == n - 1: continue
            b1, b2 = P[j], P[(j + 1) % n]
            if max(a1[1], a2[1]) < min(b1[1], b2[1]) or max(b1[1], b2[1]) < min(a1[1], a2[1]): continue
            if orient(a1, a2, b1) * orient(a1, a2, b2) < 0 and orient(b1, b2, a1) * orient(b1, b2, a2) < 0:
                hits.append((min(i, j), max(i, j)))
        active.append(i)
    print("%s: polygon %d points, proper crossings %d" % (label, n, len(hits)))
    for i, j in hits[:8]:
        print("   edge %d seg x edge %d seg, at about (%.4f,%.4f,%.4f); index gap %d; nearest wire vertex %.3e away" % (
            owner[i], owner[j], V[i][0], V[i][1], V[i][2], j - i,
            min(math.dist(V[i], (v.X, v.Y, v.Z)) for v in w.Vertexes)))
test(chains, "curve ends as evaluated (validator's way)")
snapped = [list(c) for c in chains]
for k in range(len(snapped)):
    snapped[(k + 1) % len(snapped)][0] = snapped[k][-1]
test(snapped, "edge ends snapped to one shared point")
sys.stdout.flush()
