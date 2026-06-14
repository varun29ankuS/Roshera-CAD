//! Agent-facing software renderer (AGENT-RENDER-α).
//!
//! The "screenshot for agents": a deterministic, headless, GPU-free
//! rasterizer that turns a solid into an image a vision-capable agent (or
//! a human reading a diagnostic) can actually look at. Two modes:
//!
//! * [`RenderMode::Shaded`] — headlight-shaded grayscale; "what does this
//!   part look like".
//! * [`RenderMode::FaceIds`] — every B-Rep face rendered FLAT in a
//!   distinct color, with a legend mapping color → `FaceId`. This is
//!   set-of-marks grounding for topology: an agent doesn't just see the
//!   shape, it sees *addressable* faces ("the red face is face 12 —
//!   fillet its edges"). Enabled by the tessellator's per-triangle
//!   `face_map`; flat colors are deliberate so the color → face mapping
//!   is exact under any image resampling a vision pipeline applies.
//!
//! Orthographic projection over four canonical views. Determinism matters
//! (same solid → identical bytes) so renders can be snapshot-tested and
//! diffed across kernel changes the same way volumes are.

use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};

/// EYE-1: coordinate-anchored dimensioned multi-view render.
pub mod dimensioned;

/// EYE-6: active-perception viewpoint selection (next-best-view).
pub mod viewpoint;

/// Canonical orthographic camera directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalView {
    /// Classic engineering isometric: looking along (−1, −1, −1).
    Isometric,
    /// Looking along −Y (sees the XZ plane).
    Front,
    /// Looking along −Z (sees the XY plane).
    Top,
    /// Looking along −X (sees the YZ plane).
    Right,
}

impl CanonicalView {
    /// Unit view direction (from camera toward the scene).
    pub(crate) fn direction(self) -> Vector3 {
        match self {
            CanonicalView::Isometric => {
                let inv = 1.0 / 3.0_f64.sqrt();
                Vector3::new(-inv, -inv, -inv)
            }
            CanonicalView::Front => Vector3::new(0.0, -1.0, 0.0),
            CanonicalView::Top => Vector3::new(0.0, 0.0, -1.0),
            CanonicalView::Right => Vector3::new(-1.0, 0.0, 0.0),
        }
    }

    /// A world "up" hint that is never parallel to the view direction.
    pub(crate) fn up_hint(self) -> Vector3 {
        match self {
            CanonicalView::Top => Vector3::new(0.0, 1.0, 0.0),
            _ => Vector3::new(0.0, 0.0, 1.0),
        }
    }
}

/// What the rasterizer paints per face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Headlight-shaded grayscale (normal · view), white background.
    Shaded,
    /// Flat distinct color per `FaceId` + legend. No shading, so the
    /// color → face mapping survives image resampling exactly.
    FaceIds,
    /// Depth map: nearer geometry darker (40), farthest lighter (220),
    /// background white (255). The z-buffer emitted as an image — the
    /// "binocular" channel without the inference: we own the world, so
    /// depth is read off, not reconstructed from disparity.
    Depth,
    /// Per-triangle world-space normal encoded as RGB ((n+1)/2 · 255).
    /// Surface-orientation channel of the G-buffer.
    Normals,
    /// Defect-finder for boolean/tessellation errors. Renders shaded with
    /// FRONT-FACE CULLING (so a missing or inward-flipped face shows as a
    /// see-through hole, exactly as a front-side viewer sees it), then
    /// overlays the mesh's broken edges: OPEN/boundary edges (bordered by
    /// one triangle — a hole rim) in bright RED, NON-MANIFOLD edges
    /// (bordered by 3+ triangles — overlapping/duplicate faces) in MAGENTA.
    /// `RenderFrame::{open_edges, nonmanifold_edges}` carry the counts.
    Diagnostic,
}

/// Render request.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub width: usize,
    pub height: usize,
    pub view: CanonicalView,
    pub mode: RenderMode,
    /// Tessellation quality for the underlying mesh.
    pub tessellation: TessellationParams,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            view: CanonicalView::Isometric,
            mode: RenderMode::Shaded,
            tessellation: TessellationParams::default(),
        }
    }
}

/// Raw framebuffer + legend. `pixels` is row-major RGB8, top row first.
#[derive(Debug, Clone)]
pub struct RenderFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    /// `FaceIds` mode: (face_id, rgb) for every face that received a
    /// color (whether or not it survived the depth test). Empty in
    /// `Shaded` mode.
    pub face_legend: Vec<(u32, [u8; 3])>,
    /// `Diagnostic` mode: count of OPEN (boundary) undirected mesh edges —
    /// a hole rim where a face is missing. 0 elsewhere.
    pub open_edges: usize,
    /// `Diagnostic` mode: count of NON-MANIFOLD undirected mesh edges
    /// (bordered by 3+ triangles — overlapping/duplicate faces). 0 elsewhere.
    pub nonmanifold_edges: usize,
}

impl RenderFrame {
    /// Encode the framebuffer as a PNG byte stream.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|e| format!("png header: {e}"))?;
            writer
                .write_image_data(&self.pixels)
                .map_err(|e| format!("png data: {e}"))?;
        }
        Ok(out)
    }

    /// Count of distinct non-background colors actually present in the
    /// framebuffer — the verification handle for `FaceIds` mode (should
    /// equal the number of camera-visible faces).
    pub fn distinct_foreground_colors(&self) -> usize {
        let mut seen = std::collections::BTreeSet::new();
        for px in self.pixels.chunks_exact(3) {
            if px != BACKGROUND {
                seen.insert([px[0], px[1], px[2]]);
            }
        }
        seen.len()
    }
}

const BACKGROUND: &[u8] = &[255, 255, 255];

/// Distinct, stable color for the n-th face: golden-ratio hue stepping at
/// full saturation, avoiding near-white so no face collides with the
/// background. Deterministic in the face's POSITION in the solid's sorted
/// face list (stable across runs; stable across sessions as long as the
/// face set is unchanged).
fn face_color(n: usize) -> [u8; 3] {
    const GOLDEN: f64 = 0.618_033_988_749_894_9;
    let h = (n as f64 * GOLDEN).fract() * 6.0;
    let v = 0.92_f64;
    let s = 0.85_f64;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as usize {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ]
}

/// Render one solid to a framebuffer.
///
/// Returns `None` when the solid does not exist or tessellates to an
/// empty mesh (nothing to draw is a caller-visible condition, not an
/// error).
pub fn render_solid(
    model: &BRepModel,
    solid_id: SolidId,
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    let dir = opts.view.direction();
    let up_hint = opts.view.up_hint();
    render_solid_dir(model, solid_id, dir, up_hint, opts)
}

/// Render from an ARBITRARY view direction — the engine behind both the
/// canonical views and EYE-6 orbit / next-best-view. `dir` points camera→scene;
/// `up_hint` must not be parallel to it (the caller resolves pole degeneracy,
/// e.g. switch to world-Y when `|dir·Z|` ≈ 1). `opts.view` is ignored here.
pub fn render_solid_dir(
    model: &BRepModel,
    solid_id: SolidId,
    dir: Vector3,
    up_hint: Vector3,
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    let solid = model.solids.get(solid_id)?;
    let mesh = tessellate_solid(solid, model, &opts.tessellation);
    if mesh.triangles.is_empty() {
        return None;
    }

    // Defect analysis (Diagnostic mode): weld per-face vertices by position,
    // then count how many triangles border each undirected edge. A closed
    // manifold borders every edge with exactly 2; 1 = OPEN (hole rim, a
    // missing face), 3+ = NON-MANIFOLD (overlapping/duplicate faces). The
    // representative welded vertex is an original index so endpoints project
    // through the same `px` table the triangles use.
    let mut defect_open: Vec<(usize, usize)> = Vec::new();
    let mut defect_nonmanifold: Vec<(usize, usize)> = Vec::new();
    if opts.mode == RenderMode::Diagnostic {
        use std::collections::HashMap;
        const Q: f64 = 1.0e5; // 1e-5 length weld
        let key = |p: &Point3| -> (i64, i64, i64) {
            (
                (p.x * Q).round() as i64,
                (p.y * Q).round() as i64,
                (p.z * Q).round() as i64,
            )
        };
        let mut canon: HashMap<(i64, i64, i64), usize> = HashMap::new();
        let vert_canon: Vec<usize> = mesh
            .vertices
            .iter()
            .enumerate()
            .map(|(i, v)| *canon.entry(key(&v.position)).or_insert(i))
            .collect();
        let mut edge_count: HashMap<(usize, usize), u32> = HashMap::new();
        for tri in &mesh.triangles {
            let cs = [
                vert_canon[tri[0] as usize],
                vert_canon[tri[1] as usize],
                vert_canon[tri[2] as usize],
            ];
            for k in 0..3 {
                let (a, b) = (cs[k], cs[(k + 1) % 3]);
                if a == b {
                    continue;
                }
                let e = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(e).or_insert(0) += 1;
            }
        }
        for (&e, &c) in &edge_count {
            if c == 1 {
                defect_open.push(e);
            } else if c >= 3 {
                defect_nonmanifold.push(e);
            }
        }
    }
    let open_edges = defect_open.len();
    let nonmanifold_edges = defect_nonmanifold.len();

    // Camera basis: right/up orthonormal to the view direction.
    let right = match up_hint.cross(&dir).normalize() {
        Ok(v) => v,
        Err(_) => return None,
    };
    let up = match dir.cross(&right).normalize() {
        Ok(v) => v,
        Err(_) => return None,
    };

    // Project every vertex into camera coordinates (u = right, v = up,
    // w = depth along view dir — larger w is farther).
    let cam = |p: &Point3| -> (f64, f64, f64) {
        let q = Vector3::new(p.x, p.y, p.z);
        (q.dot(&right), q.dot(&up), q.dot(&dir))
    };
    let proj: Vec<(f64, f64, f64)> = mesh.vertices.iter().map(|v| cam(&v.position)).collect();

    // Fit an orthographic window around the projected bounds with a 5%
    // margin, preserving aspect.
    let (mut u_min, mut u_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut v_min, mut v_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(u, v, _) in &proj {
        u_min = u_min.min(u);
        u_max = u_max.max(u);
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }
    let span_u = (u_max - u_min).max(1e-12);
    let span_v = (v_max - v_min).max(1e-12);
    let scale = ((opts.width as f64 * 0.9) / span_u).min((opts.height as f64 * 0.9) / span_v);
    let off_u = (opts.width as f64 - span_u * scale) * 0.5 - u_min * scale;
    let off_v = (opts.height as f64 - span_v * scale) * 0.5 - v_min * scale;
    // Pixel-space points; flip v so +up is toward the top of the image.
    let px: Vec<(f64, f64, f64)> = proj
        .iter()
        .map(|&(u, v, w)| {
            (
                u * scale + off_u,
                opts.height as f64 - (v * scale + off_v),
                w,
            )
        })
        .collect();

    // Stable face → color assignment: sorted face ids from the face_map.
    let mut face_ids: Vec<u32> = mesh.face_map.clone();
    face_ids.sort_unstable();
    face_ids.dedup();
    let face_legend: Vec<(u32, [u8; 3])> = match opts.mode {
        RenderMode::FaceIds => face_ids
            .iter()
            .enumerate()
            .map(|(n, &fid)| (fid, face_color(n)))
            .collect(),
        RenderMode::Shaded | RenderMode::Depth | RenderMode::Normals | RenderMode::Diagnostic => {
            Vec::new()
        }
    };
    let color_of_face = |fid: u32| -> [u8; 3] {
        match face_legend.binary_search_by_key(&fid, |&(f, _)| f) {
            Ok(i) => face_legend[i].1,
            Err(_) => [128, 128, 128],
        }
    };

    let mut pixels = vec![255u8; opts.width * opts.height * 3];
    let mut zbuf = vec![f64::INFINITY; opts.width * opts.height];

    for (ti, tri) in mesh.triangles.iter().enumerate() {
        let a = px[tri[0] as usize];
        let b = px[tri[1] as usize];
        let c = px[tri[2] as usize];

        // Back-face culling is deliberately OFF: a non-manifold or
        // inverted-winding diagnostic render must still show the geometry.
        let area2 = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area2.abs() < 1e-12 {
            continue;
        }

        // World-space triangle normal (unit, or zero for degenerate) —
        // used by Shaded (lambert) and Normals (RGB encoding) modes.
        let tri_normal = {
            let p0 = &mesh.vertices[tri[0] as usize].position;
            let p1 = &mesh.vertices[tri[1] as usize].position;
            let p2 = &mesh.vertices[tri[2] as usize].position;
            let e1 = Vector3::new(p1.x - p0.x, p1.y - p0.y, p1.z - p0.z);
            let e2 = Vector3::new(p2.x - p0.x, p2.y - p0.y, p2.z - p0.z);
            let n = e1.cross(&e2);
            let mag = n.magnitude();
            if mag > 1e-15 {
                Vector3::new(n.x / mag, n.y / mag, n.z / mag)
            } else {
                Vector3::new(0.0, 0.0, 0.0)
            }
        };
        // Diagnostic mode: front-face cull. A back-facing triangle is the
        // far wall (correctly hidden) OR an inward-flipped face; culling it
        // makes a missing/flipped face read as a see-through hole, matching
        // a front-side viewer (which is why such defects hide from the
        // double-sided Shaded render but show in the browser).
        if opts.mode == RenderMode::Diagnostic && tri_normal.dot(&dir) > 1.0e-9 {
            continue;
        }
        let shade_color = match opts.mode {
            RenderMode::FaceIds => {
                color_of_face(mesh.face_map.get(ti).copied().unwrap_or(u32::MAX))
            }
            RenderMode::Diagnostic => {
                let g = (60.0 + 175.0 * tri_normal.dot(&dir).abs()) as u8;
                [g, g, g]
            }
            RenderMode::Shaded => {
                // Headlight lambert, orientation-independent via abs().
                let g = (60.0 + 175.0 * tri_normal.dot(&dir).abs()) as u8;
                [g, g, g]
            }
            RenderMode::Normals => [
                ((tri_normal.x + 1.0) * 127.5) as u8,
                ((tri_normal.y + 1.0) * 127.5) as u8,
                ((tri_normal.z + 1.0) * 127.5) as u8,
            ],
            // Depth pixels are filled by the post-pass from the z-buffer;
            // the raster pass only needs to win the depth test.
            RenderMode::Depth => [0, 0, 0],
        };

        // Raster the triangle over its pixel bbox with edge functions.
        let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
        let max_x = (a.0.max(b.0).max(c.0).ceil() as usize).min(opts.width.saturating_sub(1));
        let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let max_y = (a.1.max(b.1).max(c.1).ceil() as usize).min(opts.height.saturating_sub(1));
        let inv_area = 1.0 / area2;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let pxc = x as f64 + 0.5;
                let pyc = y as f64 + 0.5;
                // Barycentric weights via edge functions (signed, then
                // normalized by the signed area so winding cancels).
                let w0 = ((b.0 - pxc) * (c.1 - pyc) - (b.1 - pyc) * (c.0 - pxc)) * inv_area;
                let w1 = ((c.0 - pxc) * (a.1 - pyc) - (c.1 - pyc) * (a.0 - pxc)) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let depth = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let idx = y * opts.width + x;
                if depth < zbuf[idx] {
                    zbuf[idx] = depth;
                    let p = idx * 3;
                    pixels[p] = shade_color[0];
                    pixels[p + 1] = shade_color[1];
                    pixels[p + 2] = shade_color[2];
                }
            }
        }
    }

    // Depth post-pass: emit the z-buffer as grayscale. Nearer = darker
    // (40), farthest geometry = lighter (220), background = white (255).
    // Normalized over the FINITE depth range so the channel always uses
    // its full contrast regardless of scene scale.
    if opts.mode == RenderMode::Depth {
        let (mut z_min, mut z_max) = (f64::INFINITY, f64::NEG_INFINITY);
        for &z in &zbuf {
            if z.is_finite() {
                z_min = z_min.min(z);
                z_max = z_max.max(z);
            }
        }
        let span = (z_max - z_min).max(1e-12);
        for (idx, &z) in zbuf.iter().enumerate() {
            let p = idx * 3;
            if z.is_finite() {
                let g = (40.0 + 180.0 * (z - z_min) / span) as u8;
                pixels[p] = g;
                pixels[p + 1] = g;
                pixels[p + 2] = g;
            }
        }
    }

    // Diagnostic overlay: stroke broken edges on top of the culled render.
    // Non-manifold first (magenta), open edges last (red) so hole rims —
    // the "missing surface" signal — are the most prominent.
    if opts.mode == RenderMode::Diagnostic {
        let (w, h) = (opts.width, opts.height);
        let mut stroke = |e: &(usize, usize), col: [u8; 3]| {
            let (a, b) = (px[e.0], px[e.1]);
            let steps = (a.0 - b.0).abs().max((a.1 - b.1).abs()).ceil().max(1.0) as i32;
            for s in 0..=steps {
                let t = s as f64 / steps as f64;
                let cx = (a.0 + (b.0 - a.0) * t).round() as i32;
                let cy = (a.1 + (b.1 - a.1) * t).round() as i32;
                for dy in -1..=1i32 {
                    for dx in -1..=1i32 {
                        let (xx, yy) = (cx + dx, cy + dy);
                        if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                            let p = ((yy as usize) * w + xx as usize) * 3;
                            pixels[p] = col[0];
                            pixels[p + 1] = col[1];
                            pixels[p + 2] = col[2];
                        }
                    }
                }
            }
        };
        for e in &defect_nonmanifold {
            stroke(e, [255, 0, 255]);
        }
        for e in &defect_open {
            stroke(e, [255, 0, 0]);
        }
    }

    Some(RenderFrame {
        width: opts.width,
        height: opts.height,
        pixels,
        face_legend,
        open_edges,
        nonmanifold_edges,
    })
}
