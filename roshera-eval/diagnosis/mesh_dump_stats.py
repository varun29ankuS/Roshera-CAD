# Reads a tess_one_face PROBE_DUMP file; reports same-way directed edges, open
# edges and (for a structured ring mesh of ROWS x N) per-row phase/direction.
import sys, math, collections
path = sys.argv[1]
n_ring = int(sys.argv[2]) if len(sys.argv) > 2 else 0
V, T = [], []
for line in open(path, encoding="utf-8", errors="replace"):
    s = line.strip()
    if s.startswith("v ("):
        V.append(tuple(float(x) for x in s[3:-1].split(",")))
    elif s.startswith("t ["):
        T.append(tuple(int(x) for x in s[3:-1].split(",")))
print("verts", len(V), "tris", len(T))
key = lambda i: tuple(round(c * 1e5) for c in V[i])
d = collections.Counter()
for t in T:
    for k in range(3):
        d[(key(t[k]), key(t[(k + 1) % 3]))] += 1
same = [e for e, c in d.items() if c > 1]
opened = [e for e in d if (e[1], e[0]) not in d]
print("same-way directed edges:", len(same), "open edges:", len(opened))
zs = collections.Counter()
for (a, b) in same[:400]:
    zs[(round(a[2] / 1e5, 1))] += 1
print("same-way by z:", sorted(zs.items())[:30])
for (a, b) in same[:6]:
    print("  e.g.", [c / 1e5 for c in a], [c / 1e5 for c in b])
if n_ring:
    rows = [V[i:i + n_ring] for i in range(0, len(V), n_ring)]
    def dist(p, q): return math.dist(p, q)
    for r in range(len(rows) - 1):
        a, b = rows[r], rows[r + 1]
        # best cyclic shift / direction aligning row r to row r+1
        best = None
        for rev in (False, True):
            bb = b[::-1] if rev else b
            for sh in range(n_ring):
                s = sum(dist(a[k], bb[(k + sh) % n_ring]) for k in range(0, n_ring, 5))
                if best is None or s < best[0]:
                    best = (s, sh, rev)
        s0 = sum(dist(a[k], b[k]) for k in range(0, n_ring, 5))
        print("row %d->%d: aligned-as-is cost %.3f; best cost %.3f at shift %d reversed %s" % (r, r + 1, s0, best[0], best[1], best[2]))
