"""The icon scenes.

Each scene is a function returning a `Scene`: a signed-distance field for the
whole object plus named material masks (which surface points are drawn in
which material). Materials: "body" (grey part), "feature" (yellow: what the
operation made), "node" (white sketch points), "go" (green: arrows, confirm),
"stop" (red: delete, close, refuse), "info" (blue). A point in no mask is body.

Two families:
  * TOOLS  -- modelling tools, a small 3D part seen in isometric.
  * GLYPHS -- interface symbols, a raised 2D glyph seen almost face-on.
"""
from dataclasses import dataclass, field
from typing import Callable, Dict

import numpy as np

from sdf import (V, arrow, box, box_frame, capsule, cyl, frustum, intersect, near,
                 sphere, strip2d, subtract, torus, union)

Field = Callable[[np.ndarray], np.ndarray]


@dataclass
class Scene:
    sdf: Field
    masks: Dict[str, Field] = field(default_factory=dict)
    camera: str = "iso"  # "iso" (tools) or "glyph" (interface symbols)


def on(f: Field, eps: float = 0.03) -> Field:
    return lambda p: near(f(p), eps)


# --------------------------------------------------------------------------
# Tools
# --------------------------------------------------------------------------

def _cube(p):
    return box(p, V(0, 0, 0), V(0.62, 0.62, 0.62))


def _all(f: Field) -> Scene:
    return Scene(f, {"feature": on(f)})


def t_box():
    return _all(lambda p: box(p, V(0, 0, 0), V(0.7, 0.7, 0.7)))


def t_sphere():
    return _all(lambda p: sphere(p, V(0, 0, 0), 0.85))


def t_cylinder():
    return _all(lambda p: cyl(p, V(0, 0, 0), 0.62, 0.75))


def t_cone():
    return _all(lambda p: frustum(p, V(0, 0, 0), 0.75, 0.02, 0.8))


def t_torus():
    return _all(lambda p: torus(p, V(0, 0, 0), 0.62, 0.28))


def t_select_object():
    return _all(_cube)


def t_select_face():
    return Scene(_cube, {"feature": lambda p: near(_cube(p)) & (p[..., 2] > 0.6)})


def t_select_edge():
    def e(p):
        return capsule(p, V(0.62, -0.62, -0.62), V(0.62, -0.62, 0.62), 0.12)
    return Scene(lambda p: union(_cube(p), e(p)), {"feature": on(e, 0.05)})


def t_select_vertex():
    def v(p):
        return sphere(p, V(0.62, -0.62, 0.62), 0.17)
    return Scene(lambda p: union(_cube(p), v(p)), {"feature": on(v, 0.05)})


def t_translate():
    def arr(p):
        return union(union(arrow(p, V(0.2, -0.2, 0.55), V(0.2, -0.2, 1.4), 0.07),
                           arrow(p, V(0.55, 0, 0), V(1.35, 0, 0), 0.07)),
                     arrow(p, V(0, -0.55, 0), V(0, -1.35, 0), 0.07))
    return Scene(lambda p: union(box(p, V(-0.15, 0.15, -0.15), V(0.5, 0.5, 0.5)), arr(p)),
                 {"go": on(arr, 0.05)})


def t_rotate():
    def ring(p):
        q = p - V(0, 0, 0.05)
        ang = np.arctan2(q[..., 1], q[..., 0])
        return np.maximum(torus(p, V(0, 0, 0.05), 0.95, 0.07), np.abs(ang - 0.9) - 1.9)
    return Scene(lambda p: union(box(p, V(0, 0, 0), V(0.45, 0.45, 0.45)), ring(p)),
                 {"go": on(ring, 0.05)})


def t_mirror():
    def a(p):
        return box(p, V(-0.55, 0, 0), V(0.3, 0.7, 0.5))

    def b(p):
        return box(p, V(0.55, 0, 0), V(0.3, 0.7, 0.5))

    def pl(p):
        return box(p, V(0, 0, 0), V(0.02, 0.95, 0.75))
    return Scene(lambda p: union(union(a(p), b(p)), pl(p)), {"feature": on(pl, 0.04)})


# Sketch tools: an upright sketch panel in the iso view, the profile in
# yellow and its points as white nodes (as every CAD sketch shows them).
_EX, _EY, _EZ = V(1, 0, 0), V(0, 0, 1), V(0, -1, 0)


def _upright(f: Field) -> Field:
    return lambda p: f(np.stack([p @ _EX, p @ _EY, p @ _EZ], -1))


def _sketch(prof: Field, pts) -> Scene:
    def plate(p):
        return box(p, V(0, 0, -0.1), V(0.95, 0.95, 0.04))

    def nodes(p):
        d = None
        for x, y in pts:
            c = sphere(p, V(x, y, 0.02), 0.12)
            d = c if d is None else union(d, c)
        return d
    whole = _upright(lambda p: union(union(plate(p), prof(p)), nodes(p)))
    return Scene(whole, {"node": _upright(on(nodes, 0.04)), "feature": _upright(on(prof, 0.04))})


def _rect2(cx, hx, hy):
    return lambda p: np.maximum(np.abs(p[..., 0] - cx) - hx, np.abs(p[..., 1]) - hy)


def _circ2(cx, r):
    return lambda p: np.linalg.norm(p[..., :2] - np.array([cx, 0.0]), axis=-1) - r


def t_sketch():
    r1, c1 = strip2d(_rect2(-0.2, 0.42, 0.34), 0.05), strip2d(_circ2(0.45, 0.3), 0.05)
    return _sketch(lambda p: union(r1(p), c1(p)),
                   [(-0.62, -0.34), (0.22, -0.34), (0.22, 0.34), (-0.62, 0.34), (0.45, 0.0)])


def t_polyline():
    def prof(p):
        return union(union(capsule(p, V(-0.6, 0.5, 0), V(-0.1, -0.4, 0), 0.06),
                           capsule(p, V(-0.1, -0.4, 0), V(0.3, 0.3, 0), 0.06)),
                     capsule(p, V(0.3, 0.3, 0), V(0.7, -0.5, 0), 0.06))
    return _sketch(prof, [(-0.6, 0.5), (-0.1, -0.4), (0.3, 0.3), (0.7, -0.5)])


def t_rectangle():
    return _sketch(strip2d(_rect2(0, 0.55, 0.4), 0.05),
                   [(-0.55, -0.4), (0.55, -0.4), (0.55, 0.4), (-0.55, 0.4)])


def t_circle():
    return _sketch(strip2d(_circ2(0, 0.5), 0.05), [(0.0, 0.0), (0.5, 0.0)])


def t_point():
    return _sketch(lambda p: np.full(p.shape[:-1], 9.0), [(0.0, 0.0)])


def t_line():
    return _sketch(lambda p: capsule(p, V(-0.6, -0.45, 0), V(0.6, 0.45, 0), 0.06),
                   [(-0.6, -0.45), (0.6, 0.45)])


def t_offset():
    # A face copied outward from the part by a distance: the yellow slab
    # hovering above the top face, with a green arrow showing the offset.
    def blk(p):
        return box(p, V(0, 0, -0.25), V(0.8, 0.8, 0.4))

    def skin(p):
        return box(p, V(0, 0, 0.55), V(0.8, 0.8, 0.06))

    def arr(p):
        return arrow(p, V(0.55, -0.55, 0.17), V(0.55, -0.55, 0.47), 0.05)
    return Scene(lambda p: union(union(blk(p), skin(p)), arr(p)),
                 {"feature": on(skin), "go": lambda p: arr(p) < 0.03})


def t_extrude():
    def base(p):
        return box(p, V(0, 0, -0.55), V(0.9, 0.9, 0.12))

    def boss(p):
        return box(p, V(0, 0, 0.1), V(0.4, 0.4, 0.55))

    def arr(p):
        return arrow(p, V(0, 0, 0.7), V(0, 0, 1.55), 0.07)
    return Scene(lambda p: union(union(base(p), boss(p)), arr(p)),
                 {"feature": on(boss), "go": lambda p: arr(p) < 0.03})


def t_revolve():
    def body(p):
        r = np.linalg.norm(p[..., :2], axis=-1)
        z = p[..., 2]
        prof = 0.32 + 0.28 * np.cos(z * 2.4) ** 2
        return np.maximum((r - prof) * 0.7, np.abs(z) - 0.8)

    def ring(p):
        q = p - V(0, 0, 0.95)
        ang = np.arctan2(q[..., 1], q[..., 0])
        return np.maximum(torus(p, V(0, 0, 0.95), 0.55, 0.05), np.abs(ang + 0.6) - 2.2)
    return Scene(lambda p: union(body(p), ring(p)),
                 {"feature": lambda p: near(body(p)) & (p[..., 2] > 0.1) & (p[..., 2] < 0.6),
                  "go": lambda p: ring(p) < 0.03})


def t_loft():
    def f(p):
        z = np.clip((p[..., 2] + 0.75) / 1.5, 0, 1)
        sq = np.maximum(np.abs(p[..., 0]), np.abs(p[..., 1])) - 0.7
        ci = np.linalg.norm(p[..., :2], axis=-1) - 0.42
        return np.maximum((sq * (1 - z) + ci * z) * 0.8, np.abs(p[..., 2]) - 0.75)
    return _all(f)


def t_sweep():
    def f(p):
        q = p - V(0.55, 0.55, -0.3)
        ang = np.arctan2(q[..., 1], q[..., 0])
        return np.maximum(torus(p, V(0.55, 0.55, -0.3), 1.0, 0.25), np.abs(ang + 2.36) - 0.9)
    return _all(f)


def t_union():
    def a(p):
        return box(p, V(-0.2, 0.25, -0.25), V(0.65, 0.55, 0.35))

    def b(p):
        return box(p, V(0.45, -0.45, 0.1), V(0.3, 0.3, 0.7))
    return Scene(lambda p: union(a(p), b(p)),
                 {"feature": lambda p: near(b(p)) & (b(p) <= a(p) + 0.01)})


def t_intersect():
    def a(p):
        return box(p, V(-0.3, 0.3, 0), V(0.55, 0.55, 0.55))

    def b(p):
        return cyl(p, V(0.25, -0.25, 0), 0.6, 0.65)
    return _all(lambda p: intersect(a(p), b(p)))


def t_subtract():
    def blk(p):
        return box(p, V(0, 0, 0), V(0.85, 0.85, 0.5))

    def cut(p):
        return box(p, V(0, 0, 0.45), V(0.28, 1.2, 0.45))
    return Scene(lambda p: subtract(blk(p), cut(p)), {"feature": on(cut)})


def t_shell():
    def outer(p):
        return box(p, V(0, 0, 0), V(0.85, 0.85, 0.55))

    def inner(p):
        return box(p, V(0, 0, 0.2), V(0.68, 0.68, 0.6))
    return Scene(lambda p: subtract(outer(p), inner(p)), {"feature": on(inner)})


def t_split():
    def lo(p):
        return box(p, V(0, 0, -0.42), V(0.75, 0.75, 0.33))

    def hi(p):
        return box(p, V(0, 0, 0.5), V(0.75, 0.75, 0.33))
    return Scene(lambda p: union(lo(p), hi(p)),
                 {"feature": lambda p: near(lo(p)) & (p[..., 2] > -0.12)})


def t_draft():
    def f(p):
        half = 0.8 - 0.3 * (p[..., 2] + 0.6) / 1.2
        return np.maximum((np.maximum(np.abs(p[..., 0]), np.abs(p[..., 1])) - half) * 0.9,
                          np.abs(p[..., 2]) - 0.6)
    return Scene(f, {"feature": lambda p: near(f(p)) & (np.abs(p[..., 2]) < 0.58)})


def _pins(centres, r, hh, z):
    def f(p):
        d = None
        for x, y in centres:
            c = cyl(p, V(x, y, z), r, hh)
            d = c if d is None else union(d, c)
        return d
    return f


def t_linear_pattern():
    def base(p):
        return box(p, V(0, 0, -0.5), V(1.0, 0.45, 0.12))
    pins = _pins([(-0.65, 0), (0, 0), (0.65, 0)], 0.24, 0.32, -0.1)
    return Scene(lambda p: union(base(p), pins(p)), {"feature": on(pins)})


def t_circular_pattern():
    def base(p):
        return cyl(p, V(0, 0, -0.5), 0.95, 0.12)
    pins = _pins([(0.62 * np.cos(k * np.pi / 3), 0.62 * np.sin(k * np.pi / 3)) for k in range(6)],
                 0.17, 0.25, -0.15)
    return Scene(lambda p: union(base(p), pins(p)), {"feature": on(pins)})


def t_rectangular_pattern():
    def base(p):
        return box(p, V(0, 0, -0.5), V(0.9, 0.9, 0.12))
    pins = _pins([(x, y) for x in (-0.45, 0.45) for y in (-0.45, 0.45)], 0.27, 0.32, -0.1)
    return Scene(lambda p: union(base(p), pins(p)), {"feature": on(pins)})


def t_copy():
    def a(p):
        return box(p, V(-0.4, 0.4, -0.2), V(0.45, 0.45, 0.45))

    def b(p):
        return box(p, V(0.45, -0.45, 0.2), V(0.45, 0.45, 0.45))
    return Scene(lambda p: union(a(p), b(p)), {"feature": lambda p: near(b(p)) & (b(p) <= a(p))})


def t_hole():
    def blk(p):
        return box(p, V(0, 0, 0), V(0.85, 0.85, 0.45))

    def hole(p):
        return cyl(p, V(0, 0, 0), 0.45, 0.8)
    return Scene(lambda p: subtract(blk(p), hole(p)), {"feature": on(hole)})


def t_thread():
    def head(p):
        q = p - V(0, 0, 0.6)
        r = np.linalg.norm(q[..., :2], axis=-1)
        ang = np.arctan2(q[..., 1], q[..., 0])
        hexr = 0.62 * np.cos(np.pi / 6) / np.cos(((ang + np.pi / 6) % (np.pi / 3)) - np.pi / 6)
        return np.maximum(r - hexr, np.abs(q[..., 2]) - 0.2)

    def shank(p):
        r = np.linalg.norm(p[..., :2], axis=-1)
        ang = np.arctan2(p[..., 1], p[..., 0])
        groove = 0.06 * np.sin(ang + p[..., 2] * 16)
        return np.maximum((r - 0.3 - groove) * 0.7, np.abs(p[..., 2] + 0.25) - 0.7)
    return Scene(lambda p: union(head(p), shank(p)),
                 {"feature": lambda p: near(shank(p)) & (p[..., 2] < 0.38)})


def t_rib():
    def a(p):
        return box(p, V(0, 0, -0.55), V(0.85, 0.75, 0.15))

    def b(p):
        return box(p, V(-0.7, 0, 0.1), V(0.15, 0.75, 0.75))

    def web(p):
        return np.maximum(box(p, V(-0.2, 0, -0.1), V(0.5, 0.06, 0.5)), (p[..., 0] + p[..., 2]) - 0.15)
    return Scene(lambda p: union(union(a(p), b(p)), web(p)), {"feature": on(web)})


def t_fillet():
    r = 0.45

    def blk(p):
        return box(p, V(0, 0, 0), V(0.8, 0.8, 0.6))

    def rounded(p):
        q = p - V(0.8 - r, 0, 0.6 - r)
        corner = (q[..., 0] > 0) & (q[..., 2] > 0)
        dc = np.maximum(np.sqrt(q[..., 0] ** 2 + q[..., 2] ** 2) - r, np.abs(p[..., 1]) - 0.8)
        return np.where(corner, dc, blk(p))
    return Scene(rounded, {"feature": lambda p: (p[..., 0] > 0.8 - r - 0.02)
                           & (p[..., 2] > 0.6 - r - 0.02) & near(rounded(p))})


def t_chamfer():
    n = V(1, 0, 1) / np.sqrt(2)

    def plane(p):
        return (p @ n) - (0.8 + 0.6 - 0.5) / np.sqrt(2)

    def f(p):
        return intersect(box(p, V(0, 0, 0), V(0.8, 0.8, 0.6)), plane(p))
    return Scene(f, {"feature": lambda p: near(plane(p)) & near(f(p))})


def t_mass_properties():
    def blk(p):
        return box(p, V(0, 0, 0), V(0.75, 0.75, 0.55))

    def com(p):
        return sphere(p, V(0.0, 0.0, 0.62), 0.22)
    return Scene(lambda p: union(blk(p), com(p)), {"feature": on(com, 0.05)})


def t_section():
    def ring(p):
        return subtract(cyl(p, V(0, 0, 0), 0.8, 0.6), cyl(p, V(0, 0, 0), 0.4, 0.8))

    def cut(p):
        # Keep the half whose cut face (normal -y) faces the camera.
        return intersect(ring(p), -p[..., 1])
    return Scene(cut, {"feature": lambda p: near(cut(p)) & (np.abs(p[..., 1]) < 0.03)})


def t_interference():
    # One part solid, the other as a ghost wireframe; the volume they share
    # -- the clash -- as a solid yellow block.
    ca, ha = V(-0.3, 0.3, -0.1), V(0.55, 0.55, 0.55)
    cb, hb = V(0.3, -0.3, 0.2), V(0.55, 0.55, 0.55)

    def a(p):
        return box(p, ca, ha)

    def b(p):
        return box(p, cb, hb)

    def clash(p):
        return intersect(a(p), b(p))

    def ghost(p):
        return box_frame(p, cb, hb, 0.035)
    return Scene(lambda p: union(union(subtract(a(p), b(p) - 0.0), clash(p)), ghost(p)),
                 {"feature": lambda p: clash(p) < 0.03, "info": lambda p: ghost(p) < 0.03})


def t_measure():
    def blk(p):
        return box(p, V(0, 0, -0.2), V(0.8, 0.55, 0.4))

    def dim(p):
        y, z = -0.55, 0.45
        line = capsule(p, V(-0.8, y, z), V(0.8, y, z), 0.045)
        ext1 = capsule(p, V(-0.8, y, 0.2), V(-0.8, y, z + 0.1), 0.035)
        ext2 = capsule(p, V(0.8, y, 0.2), V(0.8, y, z + 0.1), 0.035)
        heads = union(arrow(p, V(-0.3, y, z), V(-0.8, y, z), 0.045), arrow(p, V(0.3, y, z), V(0.8, y, z), 0.045))
        return union(union(line, union(ext1, ext2)), heads)
    return Scene(lambda p: union(blk(p), dim(p)), {"feature": lambda p: dim(p) < 0.03})


def t_datum_plane():
    def blk(p):
        return box(p, V(0, 0, -0.45), V(0.6, 0.6, 0.3))

    def pl(p):
        return box(p, V(0, 0, 0.25), V(0.95, 0.95, 0.02))
    return Scene(lambda p: union(blk(p), pl(p)), {"feature": on(pl, 0.05)})


TOOLS = {
    "box": t_box, "sphere": t_sphere, "cylinder": t_cylinder, "cone": t_cone, "torus": t_torus,
    "select-object": t_select_object, "select-face": t_select_face, "select-edge": t_select_edge,
    "select-vertex": t_select_vertex, "translate": t_translate, "rotate": t_rotate, "mirror": t_mirror,
    "sketch": t_sketch, "point": t_point, "line": t_line, "polyline": t_polyline, "rectangle": t_rectangle, "circle": t_circle,
    "extrude": t_extrude, "revolve": t_revolve, "loft": t_loft, "sweep": t_sweep,
    "union": t_union, "intersect": t_intersect, "subtract": t_subtract, "shell": t_shell,
    "offset": t_offset, "split": t_split, "draft": t_draft, "linear-pattern": t_linear_pattern,
    "circular-pattern": t_circular_pattern, "rectangular-pattern": t_rectangular_pattern,
    "copy": t_copy, "hole": t_hole, "thread": t_thread, "rib": t_rib, "fillet": t_fillet,
    "chamfer": t_chamfer, "mass-properties": t_mass_properties, "section": t_section,
    "interference": t_interference, "measure": t_measure, "datum-plane": t_datum_plane,
}


# --------------------------------------------------------------------------
# Glyphs: a 2D shape (in the unit square, y up) raised into a thin slab and
# seen almost face-on. 2D helpers return signed distance in the plane.
# --------------------------------------------------------------------------

def d_seg(p, a, b, r):
    pa = p - np.array(a)
    ba = np.array(b, dtype=float) - np.array(a)
    h = np.clip((pa @ ba) / (ba @ ba), 0, 1)
    return np.linalg.norm(pa - h[..., None] * ba, axis=-1) - r


def d_poly(p, pts, r):
    d = None
    for a, b in zip(pts, pts[1:]):
        s = d_seg(p, a, b, r)
        d = s if d is None else np.minimum(d, s)
    return d


def d_circle(p, c, r):
    return np.linalg.norm(p - np.array(c), axis=-1) - r


def d_ring(p, c, r, w):
    return np.abs(d_circle(p, c, r)) - w


def d_arc(p, c, r, w, a0, a1):
    """Ring segment from angle a0 to a1 (radians, CCW)."""
    q = p - np.array(c)
    ang = np.arctan2(q[..., 1], q[..., 0])
    mid, half = (a0 + a1) / 2, (a1 - a0) / 2
    inside = np.abs(((ang - mid + np.pi) % (2 * np.pi)) - np.pi) <= half
    ring = np.abs(np.linalg.norm(q, axis=-1) - r) - w
    e0 = np.linalg.norm(q - r * np.array([np.cos(a0), np.sin(a0)]), axis=-1) - w
    e1 = np.linalg.norm(q - r * np.array([np.cos(a1), np.sin(a1)]), axis=-1) - w
    return np.where(inside, ring, np.minimum(e0, e1))


def d_rect(p, c, h, rad=0.0):
    q = np.abs(p - np.array(c)) - (np.array(h) - rad)
    return np.linalg.norm(np.maximum(q, 0), axis=-1) + np.minimum(q.max(-1), 0) - rad


def d_tri(p, a, b, c):
    """Filled triangle (signed distance, negative inside)."""
    pts = [np.array(a, float), np.array(b, float), np.array(c, float)]
    d = None
    s = None
    for i in range(3):
        e = pts[(i + 1) % 3] - pts[i]
        w = p - pts[i]
        proj = w - e * np.clip((w @ e) / (e @ e), 0, 1)[..., None]
        dist = np.linalg.norm(proj, axis=-1)
        d = dist if d is None else np.minimum(d, dist)
        cross = e[0] * w[..., 1] - e[1] * w[..., 0]
        s = (cross > 0) if s is None else (s & (cross > 0))
    return np.where(s, -d, d)


def d_head(p, tip, direction, size):
    """A filled arrowhead with its tip at `tip` pointing along `direction`."""
    u = np.array(direction, float)
    u /= np.linalg.norm(u)
    n = np.array([-u[1], u[0]])
    t = np.array(tip, float)
    return d_tri(p, t, t - u * size + n * size * 0.75, t - u * size - n * size * 0.75)


def glyph(shapes, thickness=0.12) -> Scene:
    """`shapes`: list of (material, d2) pairs; d2 maps (..., 2) points to a
    signed distance. Each is raised into a slab; the material is the one of
    the shape the surface point belongs to (body when 'body')."""
    def slab(d2):
        return lambda p: np.maximum(d2(p[..., :2]), np.abs(p[..., 2]) - thickness)

    slabs = [(m, slab(d)) for m, d in shapes]

    def f(p):
        d = None
        for _, s in slabs:
            v = s(p)
            d = v if d is None else np.minimum(d, v)
        return d
    masks = {}
    for m, s in slabs:
        if m == "body":
            continue
        prev = masks.get(m)
        masks[m] = (lambda p, s=s, prev=prev: (s(p) < 0.03) | (prev(p) if prev else False))
    return Scene(f, masks, camera="glyph")


def G(material, d2):
    return (material, d2)


GL = {}


def _g(name, *shapes, thickness=0.12):
    GL[name] = lambda: glyph(list(shapes), thickness)


# Confirm / refuse / state
_g("check", G("go", lambda p: d_poly(p, [(-0.6, 0.0), (-0.15, -0.45), (0.65, 0.45)], 0.13)))
_g("x", G("stop", lambda p: np.minimum(d_seg(p, (-0.5, -0.5), (0.5, 0.5), 0.13),
                                        d_seg(p, (-0.5, 0.5), (0.5, -0.5), 0.13))))
_g("check-circle", G("go", lambda p: d_ring(p, (0, 0), 0.78, 0.09)),
   G("go", lambda p: d_poly(p, [(-0.38, 0.0), (-0.1, -0.28), (0.4, 0.27)], 0.1)))
_g("x-circle", G("stop", lambda p: d_ring(p, (0, 0), 0.78, 0.09)),
   G("stop", lambda p: np.minimum(d_seg(p, (-0.3, -0.3), (0.3, 0.3), 0.1), d_seg(p, (-0.3, 0.3), (0.3, -0.3), 0.1))))
_g("ban", G("stop", lambda p: d_ring(p, (0, 0), 0.75, 0.11)),
   G("stop", lambda p: d_seg(p, (-0.53, 0.53), (0.53, -0.53), 0.11)))
_g("alert", G("feature", lambda p: d_tri(p, (0, 0.82), (-0.85, -0.68), (0.85, -0.68))),
   G("ink", lambda p: d_seg(p, (0, 0.32), (0, -0.12), 0.09)),
   G("ink", lambda p: d_circle(p, (0, -0.38), 0.1)))
_g("help", G("info", lambda p: d_ring(p, (0, 0), 0.78, 0.09)),
   G("info", lambda p: np.minimum(d_arc(p, (0, 0.17), 0.24, 0.08, -1.2, 3.3),
                                  d_seg(p, (0.12, -0.03), (0.0, -0.18), 0.08))),
   G("info", lambda p: d_circle(p, (0, -0.42), 0.09)))
_g("shield-check", G("body", lambda p: np.minimum(d_rect(p, (0, 0.25), (0.62, 0.5), 0.1),
                                                   d_tri(p, (-0.62, -0.2), (0.62, -0.2), (0, -0.85)))),
   G("go", lambda p: d_poly(p, [(-0.3, 0.05), (-0.07, -0.18), (0.32, 0.25)], 0.09)))
_g("lock", G("body", lambda p: d_arc(p, (0, 0.15), 0.38, 0.09, 0, np.pi)),
   G("body", lambda p: np.minimum(d_seg(p, (-0.38, 0.15), (-0.38, -0.05), 0.09), d_seg(p, (0.38, 0.15), (0.38, -0.05), 0.09))),
   G("feature", lambda p: d_rect(p, (0, -0.4), (0.62, 0.42), 0.08)))
_g("unlock", G("body", lambda p: d_arc(p, (0.25, 0.3), 0.38, 0.09, 0, np.pi)),
   G("body", lambda p: d_seg(p, (-0.13, 0.3), (-0.13, -0.05), 0.09)),
   G("feature", lambda p: d_rect(p, (0, -0.4), (0.62, 0.42), 0.08)))

# Edit / add / delete
_g("plus", G("go", lambda p: np.minimum(d_seg(p, (-0.62, 0), (0.62, 0), 0.13), d_seg(p, (0, -0.62), (0, 0.62), 0.13))))
_g("trash", G("stop", lambda p: d_rect(p, (0, -0.22), (0.5, 0.6), 0.06)),
   G("body", lambda p: d_rect(p, (0, 0.5), (0.68, 0.09), 0.04)),
   G("body", lambda p: d_rect(p, (0, 0.66), (0.22, 0.08), 0.04)))
_g("pencil", G("feature", lambda p: d_seg(p, (-0.45, -0.45), (0.45, 0.45), 0.17)),
   G("body", lambda p: d_tri(p, (-0.72, -0.72), (-0.4, -0.62), (-0.62, -0.4))),
   G("stop", lambda p: d_seg(p, (0.48, 0.48), (0.62, 0.62), 0.17)))
_g("pen-tool", G("feature", lambda p: d_tri(p, (0, 0.75), (-0.5, -0.15), (0.5, -0.15))),
   G("body", lambda p: d_rect(p, (0, -0.42), (0.38, 0.18), 0.05)),
   G("node", lambda p: d_circle(p, (0, 0.2), 0.1)))
_g("tag", G("feature", lambda p: np.minimum(d_rect(p, (-0.15, 0), (0.55, 0.42), 0.06),
                                            d_tri(p, (0.4, 0.42), (0.4, -0.42), (0.82, 0)))),
   G("node", lambda p: d_circle(p, (0.38, 0), 0.1)))
_g("ruler", G("feature", lambda p: d_rect(p, (0, 0), (0.85, 0.3), 0.05)),
   G("ink", lambda p: np.min(np.stack([d_seg(p, (x, 0.3), (x, 0.3 - ln), 0.045)
                                       for x, ln in ((-0.6, 0.28), (-0.3, 0.16), (0, 0.28), (0.3, 0.16), (0.6, 0.28))]), axis=0)))
_g("wrench", G("body", lambda p: d_seg(p, (-0.55, -0.55), (0.25, 0.25), 0.13)),
   G("feature", lambda p: np.maximum(d_circle(p, (0.42, 0.42), 0.36), -d_rect(p, (0.6, 0.6), (0.14, 0.3)))))

# Visibility / search
_g("eye", G("body", lambda p: np.maximum(d_circle(p, (0, -0.55), 0.95), d_circle(p, (0, 0.55), 0.95))),
   G("info", lambda p: d_circle(p, (0, 0), 0.3)))
_g("eye-off", G("body", lambda p: np.maximum(d_circle(p, (0, -0.55), 0.95), d_circle(p, (0, 0.55), 0.95))),
   G("stop", lambda p: d_seg(p, (-0.72, -0.72), (0.72, 0.72), 0.1)))
_g("search", G("info", lambda p: d_ring(p, (-0.15, 0.15), 0.45, 0.11)),
   G("body", lambda p: d_seg(p, (0.18, -0.18), (0.68, -0.68), 0.14)))

# Navigation
_g("chevron-down", G("body", lambda p: d_poly(p, [(-0.55, 0.25), (0, -0.3), (0.55, 0.25)], 0.13)))
_g("chevron-up", G("body", lambda p: d_poly(p, [(-0.55, -0.25), (0, 0.3), (0.55, -0.25)], 0.13)))
_g("chevron-right", G("body", lambda p: d_poly(p, [(-0.25, -0.55), (0.3, 0), (-0.25, 0.55)], 0.13)))
_g("arrow-left", G("body", lambda p: d_seg(p, (-0.3, 0), (0.7, 0), 0.12)),
   G("go", lambda p: d_head(p, (-0.75, 0), (-1, 0), 0.5)))
_g("arrow-right", G("body", lambda p: d_seg(p, (-0.7, 0), (0.3, 0), 0.12)),
   G("go", lambda p: d_head(p, (0.75, 0), (1, 0), 0.5)))
_g("rotate-cw", G("body", lambda p: d_arc(p, (0, 0), 0.6, 0.11, -1.2, 3.6)),
   G("go", lambda p: d_head(p, (0.62, -0.55), (0.35, -1), 0.42)))
_g("rotate-ccw", G("body", lambda p: d_arc(p, (0, 0), 0.6, 0.11, -0.46, 4.3)),
   G("go", lambda p: d_head(p, (-0.62, -0.55), (-0.35, -1), 0.42)))
_g("refresh", G("body", lambda p: d_arc(p, (0, 0), 0.6, 0.11, 0.3, 2.9)),
   G("body", lambda p: d_arc(p, (0, 0), 0.6, 0.11, 3.45, 6.0)),
   G("go", lambda p: d_head(p, (-0.72, 0.05), (-0.25, -1), 0.4)),
   G("go", lambda p: d_head(p, (0.72, -0.05), (0.25, 1), 0.4)))
_g("loader", G("info", lambda p: d_arc(p, (0, 0), 0.62, 0.12, 0.2, 4.6)))

# Media
_g("play", G("go", lambda p: d_tri(p, (-0.45, 0.65), (-0.45, -0.65), (0.7, 0))))
_g("pause", G("body", lambda p: np.minimum(d_rect(p, (-0.3, 0), (0.17, 0.62), 0.05), d_rect(p, (0.3, 0), (0.17, 0.62), 0.05))))

# Objects / people / modes
_g("box-3d", G("feature", lambda p: np.minimum(np.minimum(
    d_tri(p, (0, 0.8), (-0.7, 0.42), (0, 0.04)), d_tri(p, (0, 0.8), (0, 0.04), (0.7, 0.42))),
    np.minimum(d_rect(p, (-0.35, -0.2), (0.35, 0.62)) * 0 + d_tri(p, (-0.7, 0.42), (-0.7, -0.42), (0, -0.8)),
               d_tri(p, (-0.7, 0.42), (0, -0.8), (0, 0.04))))),
   G("body", lambda p: np.minimum(d_tri(p, (0.7, 0.42), (0, 0.04), (0, -0.8)), d_tri(p, (0.7, 0.42), (0, -0.8), (0.7, -0.42)))))
_g("square", G("body", lambda p: np.abs(d_rect(p, (0, 0), (0.62, 0.62), 0.08)) - 0.08))
_g("square-dashed", G("body", lambda p: np.maximum(np.abs(d_rect(p, (0, 0), (0.62, 0.62), 0.08)) - 0.08,
                                                     -(np.abs(((p[..., 0] + p[..., 1]) * 2.2) % 1 - 0.5) - 0.2))))
_g("user", G("info", lambda p: d_circle(p, (0, 0.38), 0.3)),
   G("body", lambda p: np.maximum(d_circle(p, (0, -0.62), 0.62), -p[..., 1] - 0.7)))
_g("bot", G("body", lambda p: d_rect(p, (0, -0.12), (0.62, 0.5), 0.16)),
   G("info", lambda p: np.minimum(d_circle(p, (-0.26, -0.08), 0.12), d_circle(p, (0.26, -0.08), 0.12))),
   G("feature", lambda p: np.minimum(d_seg(p, (0, 0.38), (0, 0.62), 0.06), d_circle(p, (0, 0.7), 0.1))))
_g("terminal", G("body", lambda p: d_rect(p, (0, 0), (0.78, 0.62), 0.1)),
   G("go", lambda p: d_poly(p, [(-0.45, 0.25), (-0.2, 0.0), (-0.45, -0.25)], 0.08)),
   G("feature", lambda p: d_seg(p, (0.0, -0.28), (0.4, -0.28), 0.07)))
_g("sun", G("feature", lambda p: d_circle(p, (0, 0), 0.32)),
   G("feature", lambda p: np.min(np.stack([d_seg(p, (0.5 * np.cos(k * np.pi / 4), 0.5 * np.sin(k * np.pi / 4)),
                                                 (0.75 * np.cos(k * np.pi / 4), 0.75 * np.sin(k * np.pi / 4)), 0.07)
                                           for k in range(8)]), axis=0)))
_g("moon", G("info", lambda p: np.maximum(d_circle(p, (0, 0), 0.65), -d_circle(p, (0.35, 0.28), 0.52))))
_g("lightbulb", G("feature", lambda p: d_circle(p, (0, 0.2), 0.5)),
   G("body", lambda p: d_rect(p, (0, -0.48), (0.24, 0.2), 0.05)))
_g("clock", G("body", lambda p: d_ring(p, (0, 0), 0.75, 0.09)),
   G("feature", lambda p: d_poly(p, [(0, 0.42), (0, 0), (0.3, -0.18)], 0.08)))
_g("file", G("body", lambda p: np.minimum(d_rect(p, (-0.08, 0), (0.5, 0.75), 0.05), d_rect(p, (0.1, -0.1), (0.5, 0.65), 0.05))),
   G("feature", lambda p: np.minimum(np.minimum(d_seg(p, (-0.25, 0.2), (0.25, 0.2), 0.05), d_seg(p, (-0.25, -0.05), (0.25, -0.05), 0.05)),
                                     d_seg(p, (-0.25, -0.3), (0.1, -0.3), 0.05))))
_g("file-down", G("body", lambda p: d_rect(p, (0, 0), (0.55, 0.75), 0.06)),
   G("go", lambda p: d_seg(p, (0, 0.35), (0, -0.05), 0.08)),
   G("go", lambda p: d_head(p, (0, -0.4), (0, -1), 0.32)))
_g("list-checks", G("go", lambda p: np.minimum(d_poly(p, [(-0.75, 0.42), (-0.6, 0.28), (-0.35, 0.55)], 0.06),
                                                d_poly(p, [(-0.75, -0.18), (-0.6, -0.32), (-0.35, -0.05)], 0.06))),
   G("body", lambda p: np.minimum(d_seg(p, (-0.1, 0.42), (0.7, 0.42), 0.07), d_seg(p, (-0.1, -0.18), (0.7, -0.18), 0.07))))
_g("grid", G("body", lambda p: np.min(np.stack([d_rect(p, (x, y), (0.2, 0.2), 0.04)
                                                 for x in (-0.5, 0, 0.5) for y in (-0.5, 0, 0.5)]), axis=0)))
_g("git-merge", G("node", lambda p: np.minimum(d_circle(p, (-0.4, 0.55), 0.15), d_circle(p, (-0.4, -0.55), 0.15))),
   G("feature", lambda p: d_circle(p, (0.45, -0.1), 0.15)),
   G("body", lambda p: np.minimum(d_seg(p, (-0.4, 0.4), (-0.4, -0.4), 0.07), d_arc(p, (0.05, 0.4), 0.45, 0.07, -1.6, 3.14))))
_g("flask", G("body", lambda p: d_rect(p, (0, 0.55), (0.17, 0.3), 0.03)),
   G("info", lambda p: np.maximum(d_tri(p, (0, 1.1), (-0.75, -0.72), (0.75, -0.72)), p[..., 1] - 0.25)))
_g("droplet", G("info", lambda p: np.minimum(d_circle(p, (0, -0.2), 0.48), d_tri(p, (0, 0.8), (-0.42, -0.05), (0.42, -0.05)))))
_g("mouse-pointer", G("body", lambda p: d_tri(p, (-0.45, 0.75), (-0.45, -0.45), (0.4, 0.05))),
   G("body", lambda p: d_seg(p, (-0.1, -0.1), (0.25, -0.65), 0.12)))
_g("mouse-pointer-click", G("body", lambda p: d_tri(p, (-0.3, 0.45), (-0.3, -0.65), (0.45, -0.15))),
   G("feature", lambda p: np.min(np.stack([d_seg(p, (-0.6, 0.6), (-0.75, 0.75), 0.06), d_seg(p, (-0.3, 0.7), (-0.3, 0.9), 0.06),
                                            d_seg(p, (-0.65, 0.3), (-0.85, 0.3), 0.06)]), axis=0)))
_g("compass", G("body", lambda p: np.minimum(d_seg(p, (0, 0.55), (-0.45, -0.7), 0.08), d_seg(p, (0, 0.55), (0.45, -0.7), 0.08))),
   G("feature", lambda p: d_circle(p, (0, 0.6), 0.16)),
   G("go", lambda p: d_arc(p, (0, 0.55), 1.05, 0.05, -2.2, -0.94)))
_g("pipette", G("body", lambda p: d_seg(p, (-0.55, -0.55), (0.2, 0.2), 0.1)),
   G("feature", lambda p: d_seg(p, (0.25, 0.25), (0.55, 0.55), 0.2)))
_g("move", G("body", lambda p: np.minimum(d_seg(p, (-0.55, 0), (0.55, 0), 0.08), d_seg(p, (0, -0.55), (0, 0.55), 0.08))),
   G("go", lambda p: np.min(np.stack([d_head(p, (0.8, 0), (1, 0), 0.3), d_head(p, (-0.8, 0), (-1, 0), 0.3),
                                      d_head(p, (0, 0.8), (0, 1), 0.3), d_head(p, (0, -0.8), (0, -1), 0.3)]), axis=0)))

_g("grip", G("body", lambda p: np.min(np.stack([d_circle(p, (x, y), 0.13)
                                                for x in (-0.25, 0.25) for y in (-0.5, 0, 0.5)]), axis=0)))
_g("grip-horizontal", G("body", lambda p: np.min(np.stack([d_circle(p, (x, y), 0.13)
                                                           for x in (-0.5, 0, 0.5) for y in (-0.25, 0.25)]), axis=0)))
_g("dot", G("body", lambda p: d_circle(p, (0, 0), 0.25)))
_g("send", G("info", lambda p: np.minimum(d_tri(p, (0.8, 0.0), (-0.75, 0.7), (-0.35, 0.0)),
                                          d_tri(p, (0.8, 0.0), (-0.35, 0.0), (-0.75, -0.7)))),
   G("ink", lambda p: d_seg(p, (-0.35, 0.0), (0.3, 0.0), 0.04)))
_g("undo", G("body", lambda p: d_arc(p, (0.1, -0.1), 0.5, 0.11, -1.6, 1.6)),
   G("body", lambda p: d_seg(p, (0.1, 0.4), (-0.35, 0.4), 0.11)),
   G("go", lambda p: d_head(p, (-0.72, 0.4), (-1, 0), 0.42)))
_g("file-up", G("body", lambda p: d_rect(p, (0, 0), (0.55, 0.75), 0.06)),
   G("go", lambda p: d_seg(p, (0, -0.4), (0, 0.0), 0.08)),
   G("go", lambda p: d_head(p, (0, 0.4), (0, 1), 0.32)))
_g("notebook", G("body", lambda p: d_rect(p, (0.08, 0), (0.55, 0.75), 0.06)),
   G("ink", lambda p: np.min(np.stack([d_seg(p, (-0.62, y), (-0.38, y), 0.05) for y in (-0.45, 0, 0.45)]), axis=0)),
   G("feature", lambda p: d_seg(p, (-0.05, -0.35), (0.45, 0.35), 0.1)))
_g("tags", G("feature", lambda p: np.minimum(d_rect(p, (-0.25, 0.12), (0.5, 0.36), 0.06),
                                             d_tri(p, (0.25, 0.48), (0.25, -0.24), (0.62, 0.12)))),
   G("body", lambda p: np.maximum(np.minimum(d_rect(p, (-0.05, -0.18), (0.5, 0.36), 0.06),
                                             d_tri(p, (0.45, 0.18), (0.45, -0.54), (0.82, -0.18))),
                                  -(np.minimum(d_rect(p, (-0.25, 0.12), (0.5, 0.36), 0.06),
                                               d_tri(p, (0.25, 0.48), (0.25, -0.24), (0.62, 0.12))) - 0.08))),
   G("node", lambda p: d_circle(p, (0.22, 0.12), 0.08)))

GLYPHS = {name: make for name, make in GL.items()}
