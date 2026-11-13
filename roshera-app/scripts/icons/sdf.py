"""Signed-distance primitives for the icon scenes (z up, numpy-vectorised).

Every function takes `p`, an array of points shaped (..., 3), and returns the
signed distance to the shape (negative inside).
"""
import numpy as np


def V(*xs):
    return np.array(xs, dtype=float)


def union(a, b):
    return np.minimum(a, b)


def subtract(a, b):
    return np.maximum(a, -b)


def intersect(a, b):
    return np.maximum(a, b)


def near(d, eps=0.03):
    """Points on (within `eps` of) the surface d = 0."""
    return np.abs(d) < eps


def box(p, c, h):
    q = np.abs(p - c) - h
    return np.linalg.norm(np.maximum(q, 0), axis=-1) + np.minimum(q.max(-1), 0)


def cyl(p, c, r, hh):
    """Cylinder along z: centre c, radius r, half height hh."""
    d = p - c
    a = np.linalg.norm(d[..., :2], axis=-1) - r
    b = np.abs(d[..., 2]) - hh
    outside = np.linalg.norm(np.stack([np.maximum(a, 0), np.maximum(b, 0)], -1), axis=-1)
    return np.minimum(np.maximum(a, b), 0) + outside


def sphere(p, c, r):
    return np.linalg.norm(p - c, axis=-1) - r


def torus(p, c, big_r, r):
    q = p - c
    a = np.linalg.norm(q[..., :2], axis=-1) - big_r
    return np.sqrt(a ** 2 + q[..., 2] ** 2) - r


def frustum(p, c, r0, r1, hh):
    """Cone frustum along z: radius r0 at the bottom, r1 at the top."""
    q = p - c
    z = np.clip((q[..., 2] + hh) / (2 * hh), 0, 1)
    r = r0 + (r1 - r0) * z
    return np.maximum((np.linalg.norm(q[..., :2], axis=-1) - r) * 0.85, np.abs(q[..., 2]) - hh)


def capsule(p, a, b, r):
    pa, ba = p - a, b - a
    h = np.clip((pa @ ba) / (ba @ ba), 0, 1)
    return np.linalg.norm(pa - h[..., None] * ba, axis=-1) - r


def arrow(p, a, b, r):
    """A shaft from a toward b with a cone head ending at b."""
    d = b - a
    length = np.linalg.norm(d)
    u = d / length
    shaft = capsule(p, a, a + u * length * 0.62, r)
    q = p - (a + u * length * 0.6)
    s = q @ u
    t = np.clip(s / (length * 0.4), 0, 1)
    rad = np.linalg.norm(q - s[..., None] * u, axis=-1)
    head = np.maximum(rad - r * 2.6 * (1 - t), np.maximum(-s, s - length * 0.4)) * 0.7
    return union(shaft, head)


def strip2d(d2, w):
    """A thin raised strip along the 2D curve d2(p) = 0 lying on z = 0."""
    return lambda p: np.maximum(np.abs(d2(p)) - w, np.abs(p[..., 2]) - 0.06)


def box_frame(p, c, h, r):
    """The twelve edges of an axis-aligned box, as capsules of radius r."""
    corners = [c + h * V(sx, sy, sz) for sx in (-1, 1) for sy in (-1, 1) for sz in (-1, 1)]
    d = None
    for i, a in enumerate(corners):
        for b in corners[i + 1:]:
            if int(np.count_nonzero(np.abs(a - b) > 1e-9)) == 1:
                e = capsule(p, a, b, r)
                d = e if d is None else union(d, e)
    return d
