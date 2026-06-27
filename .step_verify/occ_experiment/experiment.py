#!/usr/bin/env python3
"""Self-contained experiment: can an LLM perceive 3D form from coarse
SDF-occupancy slice-stacks?

Implements analytic SDFs + CSG, invents 5 test solids, samples occupancy on
an 18^3 grid, and renders ASCII slice-stacks. Writes specimens.txt (blind) and
answer_key.txt (truth)."""

import math
import os

# ----------------------------------------------------------------------------
# Vector helpers (plain tuples, no numpy dependency)
# ----------------------------------------------------------------------------

def vsub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])

def vlen(a):
    return math.sqrt(a[0] * a[0] + a[1] * a[1] + a[2] * a[2])

def vlen2(a):
    return math.sqrt(a[0] * a[0] + a[1] * a[1])

# ----------------------------------------------------------------------------
# Analytic signed-distance functions (SDF). inside => sdf <= 0.
# ----------------------------------------------------------------------------

def sdf_sphere(p, center, r):
    return vlen(vsub(p, center)) - r

def sdf_box(p, center, half):
    """Axis-aligned box centered at `center` with half-extents `half`."""
    d = (abs(p[0] - center[0]) - half[0],
         abs(p[1] - center[1]) - half[1],
         abs(p[2] - center[2]) - half[2])
    outside = (max(d[0], 0.0), max(d[1], 0.0), max(d[2], 0.0))
    return vlen(outside) + min(max(d[0], max(d[1], d[2])), 0.0)

def sdf_cylinder_z(p, center, r, half_h):
    """Finite cylinder, axis parallel to +Z, centered at `center`."""
    q = vsub(p, center)
    d_radial = vlen2(q) - r
    d_axial = abs(q[2]) - half_h
    outside = (max(d_radial, 0.0), max(d_axial, 0.0))
    return min(max(d_radial, d_axial), 0.0) + math.sqrt(outside[0] ** 2 + outside[1] ** 2)

def sdf_cylinder_x(p, center, r, half_h):
    """Finite cylinder, axis parallel to +X."""
    q = vsub(p, center)
    d_radial = math.sqrt(q[1] ** 2 + q[2] ** 2) - r
    d_axial = abs(q[0]) - half_h
    outside = (max(d_radial, 0.0), max(d_axial, 0.0))
    return min(max(d_radial, d_axial), 0.0) + math.sqrt(outside[0] ** 2 + outside[1] ** 2)

def sdf_cylinder_y(p, center, r, half_h):
    """Finite cylinder, axis parallel to +Y."""
    q = vsub(p, center)
    d_radial = math.sqrt(q[0] ** 2 + q[2] ** 2) - r
    d_axial = abs(q[1]) - half_h
    outside = (max(d_radial, 0.0), max(d_axial, 0.0))
    return min(max(d_radial, d_axial), 0.0) + math.sqrt(outside[0] ** 2 + outside[1] ** 2)

def sdf_torus_z(p, center, R, r):
    """Torus in the XY plane (tube circles the +Z axis)."""
    q = vsub(p, center)
    a = vlen2(q) - R
    return math.sqrt(a * a + q[2] * q[2]) - r

def sdf_plane(p, point, normal):
    """Half-space sdf<=0 on the side the normal points away from."""
    n = vlen(normal)
    nx, ny, nz = normal[0] / n, normal[1] / n, normal[2] / n
    q = vsub(p, point)
    return q[0] * nx + q[1] * ny + q[2] * nz

# ----------------------------------------------------------------------------
# CSG combinators
# ----------------------------------------------------------------------------

def op_union(*vals):
    return min(vals)

def op_intersect(*vals):
    return max(vals)

def op_diff(a, b):
    return max(a, -b)

# ----------------------------------------------------------------------------
# The 5 invented test solids (easy -> hard).
# Each is a function p -> sdf.
# ----------------------------------------------------------------------------

# A) Hollow tube (pipe): big cylinder minus a coaxial smaller cylinder, axis +Z.
def solid_A(p):
    outer = sdf_cylinder_z(p, (0, 0, 0), 0.70, 0.78)
    inner = sdf_cylinder_z(p, (0, 0, 0), 0.42, 1.0)
    return op_diff(outer, inner)

# B) L-bracket: union of two boxes meeting at a right angle.
def solid_B(p):
    base = sdf_box(p, (0.0, -0.45, 0.0), (0.75, 0.25, 0.55))
    upright = sdf_box(p, (-0.45, 0.20, 0.0), (0.25, 0.55, 0.55))
    return op_union(base, upright)

# C) Dumbbell: two spheres joined by a thin cylinder along +X.
def solid_C(p):
    left = sdf_sphere(p, (-0.54, 0, 0), 0.32)
    right = sdf_sphere(p, (0.54, 0, 0), 0.32)
    bar = sdf_cylinder_x(p, (0, 0, 0), 0.15, 0.54)
    return op_union(left, right, bar)

# D) Box with a cylindrical through-hole drilled along +Y (a bushing block).
def solid_D(p):
    block = sdf_box(p, (0, 0, 0), (0.70, 0.70, 0.70))
    hole = sdf_cylinder_y(p, (0, 0, 0), 0.34, 1.0)
    return op_diff(block, hole)

# E) Capped torus: a torus with the top quarter sliced off by a plane,
#    AND a central post (cylinder) plugging the hole. Multi-op, hardest.
def solid_E(p):
    ring = sdf_torus_z(p, (0, 0, 0), 0.52, 0.22)
    post = sdf_cylinder_z(p, (0, 0, 0), 0.20, 0.40)
    body = op_union(ring, post)
    # slice off everything above z = +0.45 (keep sdf<=0 below the plane)
    keep_below = sdf_plane(p, (0, 0, 0.45), (0, 0, 1))
    return op_intersect(body, keep_below)

SOLIDS = [
    ("A", solid_A),
    ("B", solid_B),
    ("C", solid_C),
    ("D", solid_D),
    ("E", solid_E),
]

# ----------------------------------------------------------------------------
# Sampling + rendering
# ----------------------------------------------------------------------------

N = 18
# Domain spans [-1, 1] in each axis; solids above are authored to fill
# ~60-75% of that cube (max half-extent ~0.70 -> ~70% of the 2.0 span).
LO, HI = -1.0, 1.0

def grid_coord(i):
    # cell centers across [LO, HI]
    return LO + (i + 0.5) * (HI - LO) / N

def sample(sdf):
    """Return occ[k][j][i] booleans, axes z=k, y=j, x=i."""
    occ = []
    for k in range(N):
        z = grid_coord(k)
        layer = []
        for j in range(N):
            y = grid_coord(j)
            row = []
            for i in range(N):
                x = grid_coord(i)
                row.append(sdf((x, y, z)) <= 0.0)
            layer.append(row)
        occ.append(layer)
    return occ

def render_stack(occ):
    lines = []
    for k in range(N):
        lines.append("z=%d" % k)
        for j in range(N):
            lines.append("".join("#" if occ[k][j][i] else "." for i in range(N)))
    return "\n".join(lines)

def fill_fraction(occ):
    tot = N * N * N
    cnt = sum(1 for k in range(N) for j in range(N) for i in range(N) if occ[k][j][i])
    return cnt / tot

# ----------------------------------------------------------------------------
# Build the output files
# ----------------------------------------------------------------------------

HERE = os.path.dirname(os.path.abspath(__file__))

ANSWER = {
    "A": (
        "Hollow tube / pipe.\n"
        "  CSG: difference( cylinder_outer , cylinder_inner )\n"
        "  Primitives: outer cylinder axis=+Z r=0.70 half_h=0.78; "
        "inner coaxial cylinder r=0.42 (full height).\n"
        "  Orientation: axis vertical (+Z).\n"
        "  English: a straight circular pipe with a round bore through it."
    ),
    "B": (
        "L-bracket (right-angle gusset).\n"
        "  CSG: union( box_base , box_upright )\n"
        "  Primitives: base box half=(0.75,0.25,0.55) at (0,-0.45,0); "
        "upright box half=(0.25,0.55,0.55) at (-0.45,0.20,0).\n"
        "  Orientation: L lies in the XY plane, extruded along Z.\n"
        "  English: two slabs joined at a 90-degree corner forming an L."
    ),
    "C": (
        "Dumbbell.\n"
        "  CSG: union( sphere_left , sphere_right , cylinder_bar )\n"
        "  Primitives: spheres r=0.32 at x=-0.54 and x=+0.54; "
        "connecting cylinder axis=+X r=0.15 half_h=0.54.\n"
        "  Orientation: bar horizontal along X.\n"
        "  English: two balls joined by a thin rod -- a dumbbell."
    ),
    "D": (
        "Box with a cylindrical through-hole (bushing block).\n"
        "  CSG: difference( box , cylinder_hole )\n"
        "  Primitives: box half=(0.70,0.70,0.70) centered; "
        "hole cylinder axis=+Y r=0.34 through the full block.\n"
        "  Orientation: hole bored vertically through the cube along Y.\n"
        "  English: a solid cube with a round hole drilled straight through it."
    ),
    "E": (
        "Capped/plugged torus, top sliced flat.\n"
        "  CSG: intersect( union( torus , center_post ) , halfspace_below_z=0.45 )\n"
        "  Primitives: torus in XY plane R=0.52 tube r=0.22; "
        "central post cylinder axis=+Z r=0.20 half_h=0.40; "
        "cutting plane point=(0,0,0.45) normal=+Z (keep below).\n"
        "  Orientation: ring lies flat in XY, post fills the hole, top shaved.\n"
        "  English: a ring/donut with a central hub plug, with its top "
        "shaved off by a horizontal plane."
    ),
}

def main():
    spec_lines = []
    key_lines = []
    for label, sdf in SOLIDS:
        occ = sample(sdf)
        frac = fill_fraction(occ)
        spec_lines.append("===== SPECIMEN %s =====" % label)
        spec_lines.append(render_stack(occ))
        spec_lines.append("")  # blank separator
        key_lines.append("===== SPECIMEN %s =====" % label)
        key_lines.append(ANSWER[label])
        key_lines.append("  (fill fraction of grid: %.1f%%)" % (frac * 100))
        key_lines.append("")

    specimens = "\n".join(spec_lines).rstrip() + "\n"
    answer = "\n".join(key_lines).rstrip() + "\n"

    with open(os.path.join(HERE, "specimens.txt"), "w") as f:
        f.write(specimens)
    with open(os.path.join(HERE, "answer_key.txt"), "w") as f:
        f.write(answer)

    # Print the full specimens.txt to stdout.
    print(specimens, end="")

if __name__ == "__main__":
    main()
