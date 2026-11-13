"""Render one icon scene to an RGBA image in Roshera's CAD icon style.

Style (Varun 2026-11-13, "solidworks / fusion ... especially yellow"): a light
grey part body, the operation in one meaning colour (yellow = what the
operation made; green = motion / confirm; red = delete / refuse; blue = info;
white = sketch points), soft shading with ambient occlusion and a highlight,
thin quiet outlines. Every icon pixel is ray-marched SS x SS times and
box-filtered: smooth edges, rendered natively at each size (never stretched).
Each icon is framed to its own silhouette so it fills FILL of the tile.
"""
import numpy as np

from scenes import Scene
from sdf import V

SS = 4
T_FAR = 12.0        # rays start 6 units before the scene; past 12 they missed
FILL = 0.8          # the silhouette's larger side as a fraction of the tile
MARGIN_PX = 1       # never closer than this to the tile edge

# (shadow, lit) RGB pairs per material, plus the outline colour.
MATERIALS = {
    "body": ((0.55, 0.59, 0.66), (0.93, 0.95, 0.98)),
    "feature": ((0.78, 0.52, 0.05), (1.00, 0.86, 0.30)),
    "go": ((0.05, 0.42, 0.13), (0.30, 0.78, 0.36)),
    "stop": ((0.62, 0.10, 0.10), (0.95, 0.38, 0.33)),
    "info": ((0.10, 0.33, 0.66), (0.42, 0.68, 0.98)),
    "node": ((0.70, 0.74, 0.80), (1.00, 1.00, 1.00)),
    # Marks drawn ON another shape (the "!" of a warning, ruler ticks).
    "ink": ((0.12, 0.14, 0.18), (0.30, 0.33, 0.40)),
}
# Interface glyphs sit on the UI background, not inside a part: their neutral
# grey is darker than a part body's so a chevron or a pause bar holds on a
# light panel and still reads on a dark one.
GLYPH_BODY = ((0.30, 0.34, 0.41), (0.62, 0.66, 0.74))
OUTLINE = np.array((0.20, 0.23, 0.29))
# Paint order: a later material wins where masks overlap.
ORDER = ["feature", "info", "stop", "go", "node", "ink"]


def _camera(kind):
    if kind == "glyph":
        # Almost face-on, tipped a little so the raised glyph catches light.
        fwd = -V(0.18, -0.30, 1.0)
        up_hint = V(0, 1, 0)
        light = V(-0.35, 0.55, 0.85)
    else:
        fwd = -V(1, -1, 0.82)
        up_hint = V(0, 0, 1)
        light = V(-0.35, -0.55, 0.9)
    fwd = fwd / np.linalg.norm(fwd)
    right = np.cross(fwd, up_hint)
    right /= np.linalg.norm(right)
    up = np.cross(right, fwd)
    return fwd, right, up, light / np.linalg.norm(light)


def _march(f, n, span, cx, cy, cam):
    fwd, right, up, _ = cam
    xs = (np.arange(n) + 0.5) / n * 2 - 1
    gx, gy = np.meshgrid(xs, -xs)
    u = cx + gx * span / 2
    v = cy + gy * span / 2
    o = (u[..., None] * right + v[..., None] * up - fwd * 6).reshape(-1, 3)
    t = np.zeros(n * n)
    hit = np.zeros(n * n, bool)
    # March only the rays still travelling: a ray stops once it hits the
    # surface or has gone past everything (t > T_FAR).
    live = np.arange(n * n)
    for _ in range(260):
        if live.size == 0:
            break
        p = o[live] + fwd * t[live, None]
        d = f(p)
        h = d < 1e-3
        hit[live[h]] = True
        t[live[~h]] += np.maximum(d[~h], 2e-3)
        live = live[~h & (t[live] < T_FAR)]
    p = (o + fwd * t[:, None]).reshape(n, n, 3)
    return hit.reshape(n, n), p, u, v


def _frame(f, n, cam):
    probe = 160
    hit, _, u, v = _march(f, probe, 5.0, 0.0, 0.0, cam)
    if not hit.any():
        raise ValueError("scene renders nothing")
    cell = 5.0 / probe
    u0, u1 = u[hit].min() - cell, u[hit].max() + cell
    v0, v1 = v[hit].min() - cell, v[hit].max() + cell
    side = max(u1 - u0, v1 - v0)
    span = max(side / FILL, side * n / (n - 2 * MARGIN_PX))
    return span, (u0 + u1) / 2, (v0 + v1) / 2


def render(scene: Scene, n: int) -> np.ndarray:
    """RGBA float image (n x n x 4), straight (not premultiplied) alpha."""
    cam = _camera(scene.camera)
    fwd, _, _, light = cam
    f = scene.sdf
    span, cx, cy = _frame(f, n, cam)
    big = n * SS
    hit, p, _, _ = _march(f, big, span, cx, cy, cam)
    e = 1e-3
    nrm = np.stack([f(p + V(e, 0, 0)) - f(p - V(e, 0, 0)),
                    f(p + V(0, e, 0)) - f(p - V(0, e, 0)),
                    f(p + V(0, 0, e)) - f(p - V(0, 0, e))], -1)
    nrm /= np.linalg.norm(nrm, axis=-1, keepdims=True) + 1e-12
    diff = np.clip(nrm @ light, 0, 1)
    ao = np.zeros(hit.shape)
    for k, step in enumerate((0.05, 0.1, 0.18, 0.28)):
        ao += (step - f(p + nrm * step)) / (2 ** k)
    ao = np.clip(1 - 2.0 * ao, 0.45, 1)
    h = light - fwd
    h /= np.linalg.norm(h)
    spec = np.clip(nrm @ h, 0, 1) ** 30
    lum = np.clip((0.25 + 0.75 * diff) * ao, 0, 1)

    def shade(material):
        pair = GLYPH_BODY if (material == "body" and scene.camera == "glyph") else MATERIALS[material]
        a, b = (np.array(c) for c in pair)
        return a + (b - a) * lum[..., None]

    col = shade("body")
    for m in ORDER:
        mask = scene.masks.get(m)
        if mask is not None:
            col = np.where((hit & mask(p))[..., None], shade(m), col)
    col = np.clip(col + spec[..., None] * 0.35, 0, 1)

    sil = np.zeros_like(hit)
    crease = np.zeros_like(hit)
    for dy, dx in ((0, 1), (1, 0), (0, -1), (-1, 0)):
        nb_h = np.roll(np.roll(hit, dy, 0), dx, 1)
        nb_n = np.roll(np.roll(nrm, dy, 0), dx, 1)
        sil |= hit & ~nb_h
        crease |= hit & nb_h & ((nrm * nb_n).sum(-1) < 0.5)
    col = np.where(sil[..., None], OUTLINE, col)
    col = np.where(crease[..., None], col * 0.55 + OUTLINE * 0.45, col)

    alpha = hit.astype(float)
    rgba = np.concatenate([col * alpha[..., None], alpha[..., None]], -1)
    small = rgba.reshape(n, SS, n, SS, 4).mean(axis=(1, 3))
    a = small[..., 3:4]
    rgb = np.where(a > 0, small[..., :3] / np.maximum(a, 1e-6), 0)
    return np.concatenate([rgb, a], -1)
