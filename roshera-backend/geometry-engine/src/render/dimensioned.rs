//! EYE-1: coordinate-anchored, dimensioned multi-view render.
//!
//! [`render_solid`](super::render_solid) answers "what does it look like" from
//! one angle. EYE-1 answers "where is it, how big is it, and how do I address a
//! point on it" — the foundation an agent needs to reason about geometry rather
//! than just recognize it.
//!
//! Output = a 2×2 composite (Front / Right / Top / Isometric), each quadrant a
//! shaded render overlaid with: a coordinate triad (X red, Y green, Z blue), a
//! projected bbox wireframe, and a scale-bar ruler in world units. AND — the
//! part that matters most — every quadrant carries a [`ViewProjection`]: the
//! exact world→pixel camera transform. Coordinates are recovered from the
//! frame + this transform (`project` / `unproject_plane`), NEVER guessed from
//! pixels. That is the whole design rule: agent eyes ≠ human eyes; the numbers
//! live in the structured payload, the image is the aligned aid.
//!
//! The render is honest: it reuses the same orthographic projection and shading
//! as `render_solid`, draws no synthetic surfaces, and the dimensions reported
//! are measured off the tessellated mesh, not assumed.

use super::CanonicalView;
use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};

const BG: [u8; 3] = [250, 250, 250];
const CELL: usize = 512;
const MARGIN: f64 = 20.0;
const LABEL_H: f64 = 24.0;

/// The world→pixel transform for one quadrant of the composite. Together with
/// the basis it is a full orthographic camera matrix: a world point `p` maps to
/// composite pixel `(ox + (p·right)·scale, oy − (p·up)·scale)` at depth
/// `p·dir`. Invertible on the view plane via [`Self::unproject_plane`], which
/// is how an agent turns a pixel it is "looking at" back into a world ray.
#[derive(Debug, Clone)]
pub struct ViewProjection {
    pub view: CanonicalView,
    /// Human-facing label ("FRONT", "TOP", …).
    pub label: &'static str,
    /// Orthonormal camera basis in world space.
    pub right: Vector3,
    pub up: Vector3,
    pub dir: Vector3,
    /// World units per pixel⁻¹ (i.e. pixels per world unit).
    pub scale: f64,
    /// Pixel offsets (composite-image space).
    pub ox: f64,
    pub oy: f64,
    /// This view's cell rectangle within the composite (x, y, w, h).
    pub cell: (usize, usize, usize, usize),
}

impl ViewProjection {
    /// World point → (composite_x, composite_y, depth). Depth increases away
    /// from the camera.
    pub fn project(&self, p: &Point3) -> (f64, f64, f64) {
        let q = Vector3::new(p.x, p.y, p.z);
        let u = q.dot(&self.right);
        let v = q.dot(&self.up);
        let w = q.dot(&self.dir);
        (self.ox + u * self.scale, self.oy - v * self.scale, w)
    }

    /// Composite pixel → the world point on the view plane through the origin
    /// (depth 0). The full world ray is this point + t·dir. This is the inverse
    /// that makes coordinates recoverable from frame + query.
    pub fn unproject_plane(&self, px: f64, py: f64) -> Point3 {
        let u = (px - self.ox) / self.scale;
        let v = (self.oy - py) / self.scale;
        Point3::new(
            u * self.right.x + v * self.up.x,
            u * self.right.y + v * self.up.y,
            u * self.right.z + v * self.up.z,
        )
    }
}

/// Result of an EYE-1 render. `pixels` is row-major RGB8, top row first.
#[derive(Debug, Clone)]
pub struct MultiViewFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    /// Unit label for every reported length.
    pub units: &'static str,
    /// Axis-aligned bounding box of the tessellated solid (world space).
    pub bbox_min: Point3,
    pub bbox_max: Point3,
    /// Convenience extents: (length=X, width=Y, height=Z).
    pub dims: (f64, f64, f64),
    /// Per-quadrant camera transforms (the answer to "where is this point").
    pub views: Vec<ViewProjection>,
    /// World length represented by the scale-bar ruler in each quadrant.
    pub scale_bar_world: f64,
}

impl MultiViewFrame {
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().map_err(|e| format!("png header: {e}"))?;
            w.write_image_data(&self.pixels)
                .map_err(|e| format!("png data: {e}"))?;
        }
        Ok(out)
    }
}

/// Render the dimensioned 2×2 multi-view of a solid.
///
/// Returns `None` when the solid is absent or tessellates empty (a caller-
/// visible condition, not an error), matching `render_solid`.
pub fn render_dimensioned_multiview(
    model: &BRepModel,
    solid_id: SolidId,
    tessellation: &TessellationParams,
) -> Option<MultiViewFrame> {
    let solid = model.solids.get(solid_id)?;
    let mesh = tessellate_solid(solid, model, tessellation);
    if mesh.triangles.is_empty() {
        return None;
    }

    // Mesh AABB — the measured (not assumed) extent.
    let (mut lo, mut hi) = (
        Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
        Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for v in &mesh.vertices {
        let p = &v.position;
        lo = Point3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Point3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    let dims = (hi.x - lo.x, hi.y - lo.y, hi.z - lo.z);
    let corners = bbox_corners(&lo, &hi);

    // A scale-bar length that is a "nice" round fraction of the largest extent.
    let max_extent = dims.0.max(dims.1).max(dims.2).max(1e-9);
    let scale_bar_world = nice_number(max_extent / 4.0);

    let width = CELL * 2;
    let height = CELL * 2;
    let mut pixels = vec![0u8; width * height * 3];
    for px in pixels.chunks_exact_mut(3) {
        px.copy_from_slice(&BG);
    }
    let mut zbuf = vec![f64::INFINITY; width * height];

    // Standard layout: Front top-left, Right top-right, Top bottom-left,
    // Isometric bottom-right.
    let layout = [
        (CanonicalView::Front, "FRONT", (0usize, 0usize)),
        (CanonicalView::Right, "RIGHT", (CELL, 0)),
        (CanonicalView::Top, "TOP", (0, CELL)),
        (CanonicalView::Isometric, "ISO", (CELL, CELL)),
    ];

    let mut views = Vec::with_capacity(4);
    for (view, label, (cx0, cy0)) in layout {
        let proj = match fit_view(view, label, &corners, (cx0, cy0, CELL, CELL)) {
            Some(p) => p,
            None => continue,
        };
        raster_cell(&mesh, &proj, &mut pixels, &mut zbuf, width, height);
        draw_overlays(
            &proj,
            &lo,
            &hi,
            scale_bar_world,
            dims,
            &mut pixels,
            width,
            height,
        );
        // Cell separators.
        views.push(proj);
    }
    draw_grid_separators(&mut pixels, width, height);

    Some(MultiViewFrame {
        width,
        height,
        pixels,
        units: "mm",
        bbox_min: lo,
        bbox_max: hi,
        dims,
        views,
        scale_bar_world,
    })
}

fn bbox_corners(lo: &Point3, hi: &Point3) -> [Point3; 8] {
    [
        Point3::new(lo.x, lo.y, lo.z),
        Point3::new(hi.x, lo.y, lo.z),
        Point3::new(hi.x, hi.y, lo.z),
        Point3::new(lo.x, hi.y, lo.z),
        Point3::new(lo.x, lo.y, hi.z),
        Point3::new(hi.x, lo.y, hi.z),
        Point3::new(hi.x, hi.y, hi.z),
        Point3::new(lo.x, hi.y, hi.z),
    ]
}

/// 12 edges of the bbox as index pairs into [`bbox_corners`].
const BBOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 0),
    (4, 5),
    (5, 6),
    (6, 7),
    (7, 4),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// Build the camera transform for `view` fitting the bbox corners into the
/// cell's inner draw area (cell minus margins and the top label strip).
fn fit_view(
    view: CanonicalView,
    label: &'static str,
    corners: &[Point3; 8],
    cell: (usize, usize, usize, usize),
) -> Option<ViewProjection> {
    let dir = view.direction();
    let up_hint = view.up_hint();
    let right = up_hint.cross(&dir).normalize().ok()?;
    let up = dir.cross(&right).normalize().ok()?;

    let (mut u_min, mut u_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut v_min, mut v_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in corners {
        let q = Vector3::new(p.x, p.y, p.z);
        let (u, v) = (q.dot(&right), q.dot(&up));
        u_min = u_min.min(u);
        u_max = u_max.max(u);
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }
    let span_u = (u_max - u_min).max(1e-9);
    let span_v = (v_max - v_min).max(1e-9);

    let (x0, y0, cw, ch) = cell;
    let inner_w = cw as f64 - 2.0 * MARGIN;
    let inner_h = ch as f64 - 2.0 * MARGIN - LABEL_H;
    let scale = (inner_w / span_u).min(inner_h / span_v);

    let draw_x0 = x0 as f64 + MARGIN;
    let draw_y0 = y0 as f64 + MARGIN + LABEL_H;
    let ox = draw_x0 + (inner_w - span_u * scale) * 0.5 - u_min * scale;
    let oy = draw_y0 + (inner_h - span_v * scale) * 0.5 + v_max * scale;

    Some(ViewProjection {
        view,
        label,
        right,
        up,
        dir,
        scale,
        ox,
        oy,
        cell,
    })
}

/// Rasterize the mesh (headlight-shaded) into one cell, depth-tested and
/// clamped to the cell rectangle.
fn raster_cell(
    mesh: &crate::tessellation::TriangleMesh,
    proj: &ViewProjection,
    pixels: &mut [u8],
    zbuf: &mut [f64],
    width: usize,
    height: usize,
) {
    let (cx0, cy0, cw, ch) = proj.cell;
    let clip_x0 = cx0;
    let clip_y0 = cy0;
    let clip_x1 = (cx0 + cw).min(width);
    let clip_y1 = (cy0 + ch).min(height);

    let pp: Vec<(f64, f64, f64)> = mesh
        .vertices
        .iter()
        .map(|v| proj.project(&v.position))
        .collect();

    for tri in &mesh.triangles {
        let a = pp[tri[0] as usize];
        let b = pp[tri[1] as usize];
        let c = pp[tri[2] as usize];
        let area2 = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area2.abs() < 1e-12 {
            continue;
        }
        let p0 = &mesh.vertices[tri[0] as usize].position;
        let p1 = &mesh.vertices[tri[1] as usize].position;
        let p2 = &mesh.vertices[tri[2] as usize].position;
        let e1 = Vector3::new(p1.x - p0.x, p1.y - p0.y, p1.z - p0.z);
        let e2 = Vector3::new(p2.x - p0.x, p2.y - p0.y, p2.z - p0.z);
        let n = e1.cross(&e2);
        let mag = n.magnitude();
        let shade = if mag > 1e-15 {
            let nd = (n.x * proj.dir.x + n.y * proj.dir.y + n.z * proj.dir.z) / mag;
            (60.0 + 175.0 * nd.abs()) as u8
        } else {
            128
        };
        let color = [shade, shade, shade];

        let min_x = (a.0.min(b.0).min(c.0).floor() as i64).max(clip_x0 as i64) as usize;
        let max_x = (a.0.max(b.0).max(c.0).ceil() as i64).min(clip_x1 as i64 - 1);
        let min_y = (a.1.min(b.1).min(c.1).floor() as i64).max(clip_y0 as i64) as usize;
        let max_y = (a.1.max(b.1).max(c.1).ceil() as i64).min(clip_y1 as i64 - 1);
        if max_x < min_x as i64 || max_y < min_y as i64 {
            continue;
        }
        let inv_area = 1.0 / area2;
        for y in min_y..=(max_y as usize) {
            for x in min_x..=(max_x as usize) {
                let fx = x as f64 + 0.5;
                let fy = y as f64 + 0.5;
                let w0 = ((b.0 - fx) * (c.1 - fy) - (b.1 - fy) * (c.0 - fx)) * inv_area;
                let w1 = ((c.0 - fx) * (a.1 - fy) - (c.1 - fy) * (a.0 - fx)) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let depth = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let idx = y * width + x;
                if depth < zbuf[idx] {
                    zbuf[idx] = depth;
                    let p = idx * 3;
                    pixels[p] = color[0];
                    pixels[p + 1] = color[1];
                    pixels[p + 2] = color[2];
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_overlays(
    proj: &ViewProjection,
    lo: &Point3,
    hi: &Point3,
    scale_bar_world: f64,
    dims: (f64, f64, f64),
    pixels: &mut [u8],
    width: usize,
    height: usize,
) {
    // View label, top-left of the cell.
    let (cx0, cy0, _cw, _ch) = proj.cell;
    draw_text(
        pixels,
        width,
        height,
        cx0 as f64 + MARGIN,
        cy0 as f64 + 6.0,
        proj.label,
        [20, 20, 20],
        2,
    );

    // Bbox wireframe (dark blue).
    let corners = bbox_corners(lo, hi);
    let pc: Vec<(f64, f64, f64)> = corners.iter().map(|p| proj.project(p)).collect();
    for (i, j) in BBOX_EDGES {
        draw_line(
            pixels,
            width,
            height,
            pc[i].0,
            pc[i].1,
            pc[j].0,
            pc[j].1,
            [40, 90, 200],
        );
    }

    // Coordinate triad anchored at the bbox min corner, arms = scale_bar_world.
    let origin = *lo;
    let arm = scale_bar_world;
    let axes = [
        (Vector3::new(arm, 0.0, 0.0), [220, 40, 40], "X"),
        (Vector3::new(0.0, arm, 0.0), [40, 170, 40], "Y"),
        (Vector3::new(0.0, 0.0, arm), [50, 90, 230], "Z"),
    ];
    let o = proj.project(&origin);
    for (d, col, name) in axes {
        let tip = Point3::new(origin.x + d.x, origin.y + d.y, origin.z + d.z);
        let t = proj.project(&tip);
        draw_line(pixels, width, height, o.0, o.1, t.0, t.1, col);
        draw_text(pixels, width, height, t.0 + 2.0, t.1 - 4.0, name, col, 1);
    }

    // Scale-bar ruler, bottom-left of the cell, with end + mid ticks.
    let bar_px = scale_bar_world * proj.scale;
    let bx = cx0 as f64 + MARGIN;
    let by = (cy0 + CELL) as f64 - MARGIN;
    draw_line(pixels, width, height, bx, by, bx + bar_px, by, [20, 20, 20]);
    for t in [0.0, 0.5, 1.0] {
        let tx = bx + bar_px * t;
        draw_line(
            pixels,
            width,
            height,
            tx,
            by - 4.0,
            tx,
            by + 4.0,
            [20, 20, 20],
        );
    }
    let bar_label = format!("{} mm", fmt_num(scale_bar_world));
    draw_text(
        pixels,
        width,
        height,
        bx,
        by - 18.0,
        &bar_label,
        [20, 20, 20],
        1,
    );

    // Dimension readout (L×W×H), bottom-right of the cell.
    let dim_label = format!(
        "L{} W{} H{}",
        fmt_num(dims.0),
        fmt_num(dims.1),
        fmt_num(dims.2)
    );
    let tw = text_width(&dim_label, 1);
    draw_text(
        pixels,
        width,
        height,
        (cx0 + CELL) as f64 - MARGIN - tw,
        by - 8.0,
        &dim_label,
        [20, 20, 20],
        1,
    );
}

fn draw_grid_separators(pixels: &mut [u8], width: usize, height: usize) {
    let col = [180, 180, 180];
    for y in 0..height {
        put(pixels, width, height, CELL as i64, y as i64, col);
    }
    for x in 0..width {
        put(pixels, width, height, x as i64, CELL as i64, col);
    }
}

// ── Nice-number + formatting ────────────────────────────────────────────────

/// Largest "1/2/5 × 10ⁿ" value ≤ `x` — the classic axis-tick rounding.
fn nice_number(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return 1.0;
    }
    let exp = x.log10().floor();
    let base = 10f64.powf(exp);
    let f = x / base;
    let nf = if f >= 5.0 {
        5.0
    } else if f >= 2.0 {
        2.0
    } else {
        1.0
    };
    nf * base
}

fn fmt_num(x: f64) -> String {
    if (x - x.round()).abs() < 1e-6 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.1}")
    }
}

// ── Minimal 5×7 bitmap font ─────────────────────────────────────────────────
// Low 5 bits per row, bit4 = leftmost. Covers the glyphs EYE-1 labels need:
// digits, '.', '-', view-label letters, axis letters, and unit 'mm'.

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

fn glyph(c: char) -> Option<[u8; 7]> {
    let g: [u8; 7] = match c.to_ascii_uppercase() {
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'N' => [
            0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        ' ' => [0; 7],
        _ => return None,
    };
    Some(g)
}

fn text_width(s: &str, scale: usize) -> f64 {
    (s.chars().count() * (GLYPH_W + 1) * scale) as f64
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    x: f64,
    y: f64,
    text: &str,
    color: [u8; 3],
    scale: usize,
) {
    let mut cx = x.round() as i64;
    let cy = y.round() as i64;
    let adv = ((GLYPH_W + 1) * scale) as i64;
    for ch in text.chars() {
        if let Some(g) = glyph(ch) {
            for (row, bits) in g.iter().enumerate() {
                for col in 0..GLYPH_W {
                    if bits & (1 << (GLYPH_W - 1 - col)) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                put(
                                    pixels,
                                    width,
                                    height,
                                    cx + (col * scale + dx) as i64,
                                    cy + (row * scale + dy) as i64,
                                    color,
                                );
                            }
                        }
                    }
                }
            }
        }
        cx += adv;
    }
    let _ = (GLYPH_H, cy);
}

// ── Pixel primitives ────────────────────────────────────────────────────────

#[inline]
fn put(pixels: &mut [u8], width: usize, height: usize, x: i64, y: i64, color: [u8; 3]) {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
        return;
    }
    let p = (y as usize * width + x as usize) * 3;
    pixels[p] = color[0];
    pixels[p + 1] = color[1];
    pixels[p + 2] = color[2];
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    color: [u8; 3],
) {
    // Bresenham over the rounded endpoints.
    let mut x0 = x0.round() as i64;
    let mut y0 = y0.round() as i64;
    let x1 = x1.round() as i64;
    let y1 = y1.round() as i64;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        put(pixels, width, height, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3 as V3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    /// Build a known-good housing: box(60×60×30) + interpenetrating boss
    /// (r12, h20, base z=12) − through bore (r8). Bbox: L=60, W=60, H from
    /// z=−15 (box bottom) to z=32 (boss top) = 47.
    fn housing(model: &mut BRepModel) -> SolidId {
        let bx = sid(TopologyBuilder::new(model)
            .create_box_3d(60.0, 60.0, 30.0)
            .expect("box"));
        let boss = sid(TopologyBuilder::new(model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, 12.0),
                V3::new(0.0, 0.0, 1.0),
                12.0,
                20.0,
            )
            .expect("boss"));
        let unioned =
            boolean_operation(model, bx, boss, BooleanOp::Union, BooleanOptions::default())
                .expect("boss union");
        let bore = sid(TopologyBuilder::new(model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -20.0),
                V3::new(0.0, 0.0, 1.0),
                8.0,
                80.0,
            )
            .expect("bore"));
        boolean_operation(
            model,
            unioned,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("through bore")
    }

    #[test]
    fn dimensioned_render_reports_analytic_dims() {
        let mut model = BRepModel::new();
        let s = housing(&mut model);
        let frame = render_dimensioned_multiview(&model, s, &TessellationParams::default())
            .expect("render");

        assert_eq!(
            frame.views.len(),
            4,
            "must produce all four canonical views"
        );
        assert_eq!(frame.width, 1024);
        assert_eq!(frame.height, 1024);

        // Dimensions read off the render must match analytic intent (±tol for
        // faceting). THIS is the loop's verify-by-dimension contract.
        let (l, w, h) = frame.dims;
        assert!((l - 60.0).abs() < 0.5, "L={l} expected 60");
        assert!((w - 60.0).abs() < 0.5, "W={w} expected 60");
        assert!((h - 47.0).abs() < 0.5, "H={h} expected 47");

        // PNG must encode.
        let png = frame.to_png().expect("png");
        assert!(png.len() > 1000, "png too small: {}", png.len());
    }

    /// Emit the housing render to a PNG so it can be eyeballed (verify-by-
    /// looking). Ignored in normal runs; `cargo test … emit_eye1_png -- --ignored`.
    #[test]
    #[ignore = "writes a PNG for manual inspection"]
    fn emit_eye1_png() {
        let mut model = BRepModel::new();
        let s = housing(&mut model);
        let frame = render_dimensioned_multiview(&model, s, &TessellationParams::default())
            .expect("render");
        let png = frame.to_png().expect("png");
        std::fs::write("../_eye1_housing.png", &png).expect("write png");
        eprintln!(
            "wrote ../_eye1_housing.png ({} bytes), dims L{} W{} H{} {}",
            png.len(),
            fmt_num(frame.dims.0),
            fmt_num(frame.dims.1),
            fmt_num(frame.dims.2),
            frame.units
        );
    }

    /// Coordinate recoverability: every view's camera transform must round-trip
    /// a world point to a composite pixel inside that view's cell, and the
    /// plane-inverse must recover the in-plane component of that point. This is
    /// the "coords from frame+query, not pixels" guarantee, tested.
    #[test]
    fn camera_transform_round_trips() {
        let mut model = BRepModel::new();
        let s = housing(&mut model);
        let frame = render_dimensioned_multiview(&model, s, &TessellationParams::default())
            .expect("render");

        let probe = Point3::new(10.0, -5.0, 20.0);
        for v in &frame.views {
            let (px, py, _d) = v.project(&probe);
            let (cx0, cy0, cw, ch) = v.cell;
            assert!(
                px >= cx0 as f64
                    && px < (cx0 + cw) as f64
                    && py >= cy0 as f64
                    && py < (cy0 + ch) as f64,
                "{}: projected ({px:.1},{py:.1}) outside cell {:?}",
                v.label,
                v.cell
            );
            // Plane inverse recovers the (right,up) components of `probe`.
            let back = v.unproject_plane(px, py);
            let q = Vector3::new(probe.x, probe.y, probe.z);
            let u_expected = q.dot(&v.right);
            let v_expected = q.dot(&v.up);
            let qb = Vector3::new(back.x, back.y, back.z);
            assert!(
                (qb.dot(&v.right) - u_expected).abs() < 1e-6
                    && (qb.dot(&v.up) - v_expected).abs() < 1e-6,
                "{}: unproject did not recover in-plane coords",
                v.label
            );
        }
    }
}
