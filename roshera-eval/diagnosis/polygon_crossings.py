# Reads a tess_one_face PROBE_DUMP of ONE planar face (vertices are emitted in outer-loop
# order) and reports where the boundary polygon properly crosses itself.
import sys, math
V = []
for line in open(sys.argv[1], errors="replace"):
    s = line.strip()
    if s.startswith("v ("):
        V.append(tuple(float(x) for x in s[3:-1].split(",")))
n = len(V)
# Newell normal + axes
nx = ny = nz = 0.0
for i in range(n):
    a, b = V[i], V[(i + 1) % n]
    nx += (a[1] - b[1]) * (a[2] + b[2]); ny += (a[2] - b[2]) * (a[0] + b[0]); nz += (a[0] - b[0]) * (a[1] + b[1])
m = math.sqrt(nx * nx + ny * ny + nz * nz); N = (nx / m, ny / m, nz / m)
ref = (1, 0, 0) if abs(N[0]) < 0.9 else (0, 1, 0)
cross = lambda a, b: (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])
U = cross(N, ref); lu = math.sqrt(sum(c * c for c in U)); U = tuple(c / lu for c in U); W = cross(N, U)
P = [(sum(p[k] * U[k] for k in range(3)), sum(p[k] * W[k] for k in range(3))) for p in V]
print("vertices", n, "normal", tuple(round(c, 5) for c in N))
seg = [math.dist(P[i], P[(i + 1) % n]) for i in range(n)]
print("segment length min %.3e max %.3e; zero-length: %d" % (min(seg), max(seg), sum(1 for s in seg if s < 1e-9)))
def orient(a, b, c): return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
hits = []
for i in range(n):
    a1, a2 = P[i], P[(i + 1) % n]
    for j in range(i + 2, n):
        if i == 0 and j == n - 1: continue
        b1, b2 = P[j], P[(j + 1) % n]
        if max(a1[0], a2[0]) < min(b1[0], b2[0]) or max(b1[0], b2[0]) < min(a1[0], a2[0]): continue
        if max(a1[1], a2[1]) < min(b1[1], b2[1]) or max(b1[1], b2[1]) < min(a1[1], a2[1]): continue
        d1, d2, d3, d4 = orient(a1, a2, b1), orient(a1, a2, b2), orient(b1, b2, a1), orient(b1, b2, a2)
        if d1 * d2 < 0 and d3 * d4 < 0:
            hits.append((i, j))
print("proper crossings:", len(hits))
for i, j in hits[:12]:
    print("  seg %d (%s -> %s) x seg %d (%s -> %s); index gap %d; lens %.2e %.2e" % (
        i, tuple(round(c, 5) for c in V[i]), tuple(round(c, 5) for c in V[(i + 1) % n]),
        j, tuple(round(c, 5) for c in V[j]), tuple(round(c, 5) for c in V[(j + 1) % n]), j - i, seg[i], seg[j]))
