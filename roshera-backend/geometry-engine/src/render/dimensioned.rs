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
use serde::Serialize;

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
    /// EYE-3 analytics, MEASURED off the same tessellated mesh (so the overlay
    /// and the numbers can never disagree): solid volume, surface area, and the
    /// volume centroid. These match the kernel's mass-properties query within
    /// faceting tolerance — that agreement is the visual⇄numeric self-check.
    pub volume: f64,
    pub surface_area: f64,
    pub centroid: Point3,
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

    // EYE-3 analytics from the mesh (divergence-theorem volume/centroid + summed
    // triangle area). Same mesh the views are drawn from → overlay and numbers
    // are inherently consistent.
    let (volume, surface_area, centroid) = mesh_analytics(&mesh);

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
            (volume, surface_area, centroid),
            &mut pixels,
            width,
            height,
        );
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
        volume,
        surface_area,
        centroid,
    })
}

// ── EYE-5: ambiguity / coverage protocol ────────────────────────────────────

/// What the 4 standard EYE-1 views actually let an agent SEE — so it never
/// silently assumes it saw the whole part. `unseen_faces` are faces with zero
/// visible pixels in iso/front/top/right (back faces, deep pockets, internal
/// voids); their existence is the signal to request another angle rather than
/// reason about geometry that was never shown.
#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    pub total_faces: usize,
    /// Face ids visible in ≥1 of the 4 standard views.
    pub seen_faces: Vec<u32>,
    /// Face ids visible in NONE of the 4 standard views.
    pub unseen_faces: Vec<u32>,
    /// Faces seen in exactly one view — low-confidence (a single grazing angle).
    pub single_view_faces: Vec<u32>,
    /// seen / total.
    pub coverage_fraction: f64,
    /// Human-facing ambiguity flags ("N faces unseen — request another angle").
    pub notes: Vec<String>,
}

/// Build the coverage report by rendering FaceIds across the 4 standard views
/// and recording which face colors actually reach the framebuffer. Honest: a
/// face counts as "seen" only if it painted ≥1 pixel from some standard view.
pub fn coverage_report(
    model: &BRepModel,
    solid_id: SolidId,
    tessellation: &TessellationParams,
) -> Option<CoverageReport> {
    use super::{render_solid, RenderMode, RenderOptions};
    use std::collections::{BTreeSet, HashMap};

    let views = [
        CanonicalView::Isometric,
        CanonicalView::Front,
        CanonicalView::Top,
        CanonicalView::Right,
    ];

    let mut all_faces: BTreeSet<u32> = BTreeSet::new();
    let mut view_count: HashMap<u32, usize> = HashMap::new();

    for view in views {
        let frame = render_solid(
            model,
            solid_id,
            &RenderOptions {
                width: 512,
                height: 512,
                view,
                mode: RenderMode::FaceIds,
                tessellation: tessellation.clone(),
            },
        )?;
        // The legend covers EVERY face (visible or not) → the full face set.
        let by_color: HashMap<[u8; 3], u32> =
            frame.face_legend.iter().map(|&(f, c)| (c, f)).collect();
        for &(fid, _) in &frame.face_legend {
            all_faces.insert(fid);
        }
        // Faces that actually painted a pixel this view (flat colors, no AA →
        // exact color↔face mapping).
        let mut seen_here: BTreeSet<u32> = BTreeSet::new();
        for px in frame.pixels.chunks_exact(3) {
            if px == [255, 255, 255] {
                continue; // background
            }
            if let Some(&fid) = by_color.get(&[px[0], px[1], px[2]]) {
                seen_here.insert(fid);
            }
        }
        for fid in seen_here {
            *view_count.entry(fid).or_insert(0) += 1;
        }
    }

    let total = all_faces.len();
    if total == 0 {
        return None;
    }
    let seen_faces: Vec<u32> = view_count.keys().copied().collect();
    let mut seen_sorted = seen_faces.clone();
    seen_sorted.sort_unstable();
    let unseen_faces: Vec<u32> = all_faces
        .iter()
        .copied()
        .filter(|f| !view_count.contains_key(f))
        .collect();
    let single_view_faces: Vec<u32> = {
        let mut v: Vec<u32> = view_count
            .iter()
            .filter(|(_, &c)| c == 1)
            .map(|(&f, _)| f)
            .collect();
        v.sort_unstable();
        v
    };
    let coverage_fraction = seen_sorted.len() as f64 / total as f64;

    let mut notes = Vec::new();
    if !unseen_faces.is_empty() {
        notes.push(format!(
            "{} of {} faces are not visible in any of the 4 standard views \
             (iso/front/top/right) — request another camera angle before \
             reasoning about them",
            unseen_faces.len(),
            total
        ));
    }
    if !single_view_faces.is_empty() {
        notes.push(format!(
            "{} face(s) seen from only one view (low confidence)",
            single_view_faces.len()
        ));
    }
    if unseen_faces.is_empty() {
        notes.push("all faces visible across the 4 standard views".to_string());
    }

    Some(CoverageReport {
        total_faces: total,
        seen_faces: seen_sorted,
        unseen_faces,
        single_view_faces,
        coverage_fraction,
        notes,
    })
}

// ── EYE-2: section / clip-plane render ──────────────────────────────────────

/// A dimensioned cross-section render: the solid cut by a plane, the cut face
/// drawn TRUE-SHAPE (face-on, looking along the plane normal), with the section
/// area + in-plane extents measured off the cap geometry. Like EYE-1 it carries
/// its world→pixel transform (`right`/`up`/`scale`/`ox`/`oy`) so a point on the
/// section is recoverable from frame + query, not guessed.
#[derive(Debug, Clone)]
pub struct SectionFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    pub units: &'static str,
    pub plane_origin: Point3,
    pub plane_normal: Vector3,
    /// Total cross-section area (sum of cap triangle areas), MEASURED.
    pub section_area: f64,
    /// In-plane extents of the section (along `right`, along `up`).
    pub extent_u: f64,
    pub extent_v: f64,
    // World→pixel transform for the face-on view (camera matrix).
    pub right: Vector3,
    pub up: Vector3,
    pub scale: f64,
    pub ox: f64,
    pub oy: f64,
}

impl SectionFrame {
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

    /// World point → section-image pixel (the section's camera matrix).
    pub fn project(&self, p: &Point3) -> (f64, f64) {
        let q = Vector3::new(p.x, p.y, p.z);
        (
            self.ox + q.dot(&self.right) * self.scale,
            self.oy - q.dot(&self.up) * self.scale,
        )
    }
}

/// Render the cross-section of `solid` cut by the plane (origin, normal).
/// Reuses the kernel's `section_solid_by_plane` (the cap triangulation is the
/// source of truth) and draws it true-shape. Returns `None` if the plane
/// misses the solid or the basis is degenerate.
pub fn render_section(
    model: &BRepModel,
    solid_id: SolidId,
    plane_origin: Point3,
    plane_normal: Vector3,
    tolerance: crate::math::Tolerance,
) -> Option<SectionFrame> {
    use crate::operations::section::section_solid_by_plane;

    let caps =
        section_solid_by_plane(model, solid_id, plane_origin, plane_normal, tolerance).ok()?;
    if caps.is_empty() {
        return None;
    }

    let dir = plane_normal.normalize().ok()?;
    let up_hint = if dir.z.abs() > 0.9 {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    };
    let right = up_hint.cross(&dir).normalize().ok()?;
    let up = dir.cross(&right).normalize().ok()?;

    // Project all cap vertices to (u, v); accumulate area in 3D.
    let mut u_min = f64::INFINITY;
    let mut u_max = f64::NEG_INFINITY;
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    let mut section_area = 0.0;
    for cap in &caps {
        for v in &cap.vertices {
            let q = Vector3::new(v.x, v.y, v.z);
            let (cu, cv) = (q.dot(&right), q.dot(&up));
            u_min = u_min.min(cu);
            u_max = u_max.max(cu);
            v_min = v_min.min(cv);
            v_max = v_max.max(cv);
        }
        for tri in &cap.indices {
            let a = &cap.vertices[tri[0] as usize];
            let b = &cap.vertices[tri[1] as usize];
            let c = &cap.vertices[tri[2] as usize];
            let e1 = Vector3::new(b.x - a.x, b.y - a.y, b.z - a.z);
            let e2 = Vector3::new(c.x - a.x, c.y - a.y, c.z - a.z);
            section_area += e1.cross(&e2).magnitude() * 0.5;
        }
    }
    let span_u = (u_max - u_min).max(1e-9);
    let span_v = (v_max - v_min).max(1e-9);

    let w = CELL;
    let h = CELL;
    let inner_w = w as f64 - 2.0 * MARGIN;
    let inner_h = h as f64 - 2.0 * MARGIN - LABEL_H;
    let scale = (inner_w / span_u).min(inner_h / span_v);
    let draw_x0 = MARGIN;
    let draw_y0 = MARGIN + LABEL_H;
    let ox = draw_x0 + (inner_w - span_u * scale) * 0.5 - u_min * scale;
    let oy = draw_y0 + (inner_h - span_v * scale) * 0.5 + v_max * scale;

    let mut pixels = vec![0u8; w * h * 3];
    for px in pixels.chunks_exact_mut(3) {
        px.copy_from_slice(&BG);
    }

    let project = |p: &Point3| -> (f64, f64) {
        let q = Vector3::new(p.x, p.y, p.z);
        (ox + q.dot(&right) * scale, oy - q.dot(&up) * scale)
    };

    // Fill cap triangles (steel) then stroke their boundary edges (darker).
    let fill = [70, 110, 170];
    let stroke = [30, 50, 90];
    for cap in &caps {
        for tri in &cap.indices {
            let a = project(&cap.vertices[tri[0] as usize]);
            let b = project(&cap.vertices[tri[1] as usize]);
            let c = project(&cap.vertices[tri[2] as usize]);
            fill_tri(&mut pixels, w, h, a, b, c, fill);
        }
        // Outline: stroke every edge that belongs to exactly one triangle
        // (the section boundary, incl. inner holes).
        use std::collections::HashMap;
        let mut edge_count: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &cap.indices {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let e = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(e).or_insert(0) += 1;
            }
        }
        for (&(i, j), &cnt) in &edge_count {
            if cnt == 1 {
                let a = project(&cap.vertices[i as usize]);
                let b = project(&cap.vertices[j as usize]);
                draw_line(&mut pixels, w, h, a.0, a.1, b.0, b.1, stroke);
            }
        }
    }

    // Label + dimensions + area + scale bar.
    draw_text(&mut pixels, w, h, MARGIN, 6.0, "SECTION", [20, 20, 20], 2);
    let dim_label = format!("{} x {} mm", fmt_num(span_u), fmt_num(span_v));
    draw_text(
        &mut pixels,
        w,
        h,
        MARGIN,
        h as f64 - MARGIN + 4.0,
        &dim_label,
        [20, 20, 20],
        1,
    );
    let area_label = format!("A {} mm2", fmt_num(section_area));
    let aw = text_width(&area_label, 1);
    draw_text(
        &mut pixels,
        w,
        h,
        w as f64 - MARGIN - aw,
        h as f64 - MARGIN + 4.0,
        &area_label,
        [90, 30, 110],
        1,
    );
    let bar_world = nice_number(span_u.max(span_v) / 4.0);
    let bar_px = bar_world * scale;
    let bx = MARGIN;
    let by = h as f64 - MARGIN - 14.0;
    draw_line(&mut pixels, w, h, bx, by, bx + bar_px, by, [20, 20, 20]);
    for t in [0.0, 0.5, 1.0] {
        let tx = bx + bar_px * t;
        draw_line(&mut pixels, w, h, tx, by - 4.0, tx, by + 4.0, [20, 20, 20]);
    }
    draw_text(
        &mut pixels,
        w,
        h,
        bx,
        by - 16.0,
        &format!("{} mm", fmt_num(bar_world)),
        [20, 20, 20],
        1,
    );

    Some(SectionFrame {
        width: w,
        height: h,
        pixels,
        units: "mm",
        plane_origin,
        plane_normal: dir,
        section_area,
        extent_u: span_u,
        extent_v: span_v,
        right,
        up,
        scale,
        ox,
        oy,
    })
}

/// Flat single-color triangle fill (no depth; section caps are coplanar).
fn fill_tri(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    a: (f64, f64),
    b: (f64, f64),
    c: (f64, f64),
    color: [u8; 3],
) {
    let area2 = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    if area2.abs() < 1e-12 {
        return;
    }
    let inv = 1.0 / area2;
    let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
    let max_x = (a.0.max(b.0).max(c.0).ceil() as usize).min(width.saturating_sub(1));
    let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
    let max_y = (a.1.max(b.1).max(c.1).ceil() as usize).min(height.saturating_sub(1));
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let fx = x as f64 + 0.5;
            let fy = y as f64 + 0.5;
            let w0 = ((b.0 - fx) * (c.1 - fy) - (b.1 - fy) * (c.0 - fx)) * inv;
            let w1 = ((c.0 - fx) * (a.1 - fy) - (c.1 - fy) * (a.0 - fx)) * inv;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let p = (y * width + x) * 3;
            pixels[p] = color[0];
            pixels[p + 1] = color[1];
            pixels[p + 2] = color[2];
        }
    }
}

/// Volume (divergence theorem over the welded mesh), surface area (summed
/// triangle areas), and the volume centroid — all from the tessellated mesh.
/// For a watertight, consistently-wound mesh these equal the polyhedron's exact
/// values, and track the kernel's analytic mass properties within faceting.
fn mesh_analytics(mesh: &crate::tessellation::TriangleMesh) -> (f64, f64, Point3) {
    let mut vol6 = 0.0; // 6×volume
    let mut area2 = 0.0; // 2×area
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut cz = 0.0;
    for tri in &mesh.triangles {
        let a = &mesh.vertices[tri[0] as usize].position;
        let b = &mesh.vertices[tri[1] as usize].position;
        let c = &mesh.vertices[tri[2] as usize].position;
        // Signed volume of tet (origin, a, b, c) = a · (b × c).
        let cross = Vector3::new(
            b.y * c.z - b.z * c.y,
            b.z * c.x - b.x * c.z,
            b.x * c.y - b.y * c.x,
        );
        let sv = a.x * cross.x + a.y * cross.y + a.z * cross.z; // 6× tet volume
        vol6 += sv;
        // Volume centroid: Σ (tet_centroid · 6·tetVol); tet_centroid = (a+b+c)/4
        // (origin contributes 0). Divide by 4 and by total at the end.
        cx += sv * (a.x + b.x + c.x);
        cy += sv * (a.y + b.y + c.y);
        cz += sv * (a.z + b.z + c.z);
        // Triangle area = ½|(b−a)×(c−a)|.
        let e1 = Vector3::new(b.x - a.x, b.y - a.y, b.z - a.z);
        let e2 = Vector3::new(c.x - a.x, c.y - a.y, c.z - a.z);
        let n = e1.cross(&e2);
        area2 += n.magnitude();
    }
    let volume = (vol6 / 6.0).abs();
    let surface_area = area2 * 0.5;
    let centroid = if vol6.abs() > 1e-12 {
        // cx accumulated sv·Σpos; centroid = (Σ sv·(a+b+c)) / (4 · Σsv).
        Point3::new(cx / (4.0 * vol6), cy / (4.0 * vol6), cz / (4.0 * vol6))
    } else {
        Point3::new(0.0, 0.0, 0.0)
    };
    (volume, surface_area, centroid)
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
    analytics: (f64, f64, Point3),
    pixels: &mut [u8],
    width: usize,
    height: usize,
) {
    let (volume, surface_area, centroid) = analytics;
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

    // EYE-3: centroid marker (magenta crosshair + 'C') projected into this view.
    let cm = [200, 0, 200];
    let (cpx, cpy, _) = proj.project(&centroid);
    draw_line(pixels, width, height, cpx - 7.0, cpy, cpx + 7.0, cpy, cm);
    draw_line(pixels, width, height, cpx, cpy - 7.0, cpx, cpy + 7.0, cm);
    draw_text(pixels, width, height, cpx + 4.0, cpy + 4.0, "C", cm, 1);

    // EYE-3: measured volume + surface-area readout, top-right of the cell.
    let v_label = format!("V {}", fmt_num(volume));
    let a_label = format!("A {}", fmt_num(surface_area));
    let vw = text_width(&v_label, 1);
    let aw = text_width(&a_label, 1);
    draw_text(
        pixels,
        width,
        height,
        (cx0 + CELL) as f64 - MARGIN - vw,
        cy0 as f64 + 8.0,
        &v_label,
        [90, 30, 110],
        1,
    );
    draw_text(
        pixels,
        width,
        height,
        (cx0 + CELL) as f64 - MARGIN - aw,
        cy0 as f64 + 20.0,
        &a_label,
        [90, 30, 110],
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
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
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

// ── Dimension callouts ──────────────────────────────────────────────────────

/// One callout to draw: a world anchor point and a (already ASCII-safe) label.
/// Built from the analytic dimension table by the caller — kept as a plain
/// tuple so this render module stays decoupled from `readable::dimensions`.
pub type Callout = ([f64; 3], String);

/// Overlay analytic dimension callouts onto an already-rendered multi-view
/// frame. For each quadrant, every callout whose world anchor projects into
/// that cell is drawn as a tick at the anchor, a leader line, and the label —
/// with greedy vertical de-overlap so labels don't stack on top of each other.
///
/// The anchor is projected through the SAME `ViewProjection` the frame carries,
/// so the drawn callout and the structured dimension value can never disagree
/// (the picture is the table, placed). Labels must be ASCII (the 5×7 font has
/// no Ø/° glyphs); unknown glyphs are silently skipped.
pub fn draw_dimension_callouts(frame: &mut MultiViewFrame, callouts: &[Callout]) {
    const DIM_COLOR: [u8; 3] = [210, 105, 0]; // orange — distinct from triad/bbox
    const SCALE: usize = 2;
    let w = frame.width;
    let h = frame.height;
    let th = (GLYPH_H * SCALE) as f64;
    // Clone the (cheap) camera list so we can mutate `frame.pixels` in the loop.
    let views = frame.views.clone();
    for proj in &views {
        let (cx0, cy0, cw, ch) = proj.cell;
        let (x_lo, y_lo) = (cx0 as f64, cy0 as f64);
        let (x_hi, y_hi) = ((cx0 + cw) as f64, (cy0 + ch) as f64);
        let mut placed: Vec<(f64, f64, f64, f64)> = Vec::new();
        for (anchor, label) in callouts {
            let p = Point3::new(anchor[0], anchor[1], anchor[2]);
            let (px, py, _depth) = proj.project(&p);
            if px < x_lo || px >= x_hi || py < y_lo || py >= y_hi {
                continue; // anchor not visible in this quadrant
            }
            let tw = text_width(label, SCALE);
            // Default label box up-right of the anchor; flip in-bounds.
            let mut lx = px + 6.0;
            let mut ly = py - 6.0 - th;
            if lx + tw > x_hi - 2.0 {
                lx = (px - 6.0 - tw).max(x_lo + 2.0);
            }
            if ly < y_lo + 2.0 {
                ly = py + 6.0;
            }
            // Greedy vertical de-overlap against already-placed labels.
            let mut guard = 0;
            loop {
                let b = (lx, ly, lx + tw, ly + th);
                let hit = placed
                    .iter()
                    .any(|q| !(b.2 < q.0 || b.0 > q.2 || b.3 < q.1 || b.1 > q.3));
                if !hit || guard > 40 || ly + 2.0 * th > y_hi - 2.0 {
                    break;
                }
                ly += th + 2.0;
                guard += 1;
            }
            placed.push((lx, ly, lx + tw, ly + th));
            // Anchor tick (+), leader to the label, then the label.
            draw_line(
                &mut frame.pixels,
                w,
                h,
                px - 3.0,
                py,
                px + 3.0,
                py,
                DIM_COLOR,
            );
            draw_line(
                &mut frame.pixels,
                w,
                h,
                px,
                py - 3.0,
                px,
                py + 3.0,
                DIM_COLOR,
            );
            draw_line(&mut frame.pixels, w, h, px, py, lx, ly + th, DIM_COLOR);
            draw_text(&mut frame.pixels, w, h, lx, ly, label, DIM_COLOR, SCALE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Tolerance;
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
    #[ignore = "writes a PNG for manual inspection"]
    fn emit_dimensioned_callouts_png() {
        let mut m = BRepModel::new();
        let part = housing(&mut m);
        let mut frame =
            render_dimensioned_multiview(&m, part, &TessellationParams::default()).expect("frame");
        let dims = crate::readable::extract_dimensions(&m, part);
        let callouts: Vec<Callout> = dims
            .iter()
            .map(|d| {
                let ascii: String = d
                    .label
                    .chars()
                    .map(|c| match c {
                        'Ø' => 'D',
                        '∠' => 'A',
                        '°' => ' ',
                        o => o,
                    })
                    .collect();
                (d.anchor, ascii)
            })
            .collect();
        draw_dimension_callouts(&mut frame, &callouts);
        let png = frame.to_png().expect("png");
        std::fs::write("_dimensioned_callouts.png", png).expect("write");
    }

    fn count_dim_pixels(frame: &MultiViewFrame) -> usize {
        frame
            .pixels
            .chunks_exact(3)
            .filter(|px| px[0] == 210 && px[1] == 105 && px[2] == 0)
            .count()
    }

    #[test]
    fn dimension_callouts_draw_and_project_into_views() {
        let mut m = BRepModel::new();
        let part = housing(&mut m);
        let mut frame =
            render_dimensioned_multiview(&m, part, &TessellationParams::default()).expect("frame");
        let before = count_dim_pixels(&frame);
        assert_eq!(before, 0, "no dim-colored pixels before drawing");

        // Anchors on the part: boss top, a bbox extent, the bore axis.
        let callouts = vec![
            ([0.0_f64, 0.0, 32.0], "D24.00".to_string()),
            ([30.0, 0.0, 0.0], "X 60.00".to_string()),
            ([8.0, 0.0, 0.0], "D16.00".to_string()),
        ];
        draw_dimension_callouts(&mut frame, &callouts);
        let after = count_dim_pixels(&frame);
        assert!(after > before, "callouts drew no pixels (after {after})");
    }

    #[test]
    fn callout_label_position_recovers_anchor_via_camera() {
        // Recoverability: the pixel a callout is anchored at must unproject
        // (through the SAME ViewProjection) back onto the anchor's view-plane
        // coordinates — the drawn mark and the structured value agree by
        // construction, never read off pixels.
        let mut m = BRepModel::new();
        let part = housing(&mut m);
        let frame =
            render_dimensioned_multiview(&m, part, &TessellationParams::default()).expect("frame");
        let anchor = Point3::new(0.0, 0.0, 32.0);
        for proj in &frame.views {
            let (px, py, _) = proj.project(&anchor);
            let back = proj.unproject_plane(px, py);
            // unproject lands on the view plane: its right/up components must
            // match the anchor's.
            let want_u =
                anchor.x * proj.right.x + anchor.y * proj.right.y + anchor.z * proj.right.z;
            let want_v = anchor.x * proj.up.x + anchor.y * proj.up.y + anchor.z * proj.up.z;
            let got_u = back.x * proj.right.x + back.y * proj.right.y + back.z * proj.right.z;
            let got_v = back.x * proj.up.x + back.y * proj.up.y + back.z * proj.up.z;
            assert!(
                (want_u - got_u).abs() < 1e-6,
                "{}: u not recovered",
                proj.label
            );
            assert!(
                (want_v - got_v).abs() < 1e-6,
                "{}: v not recovered",
                proj.label
            );
        }
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

    /// EYE-3 self-check: the analytics OVERLAID on the render (volume, surface
    /// area, centroid — measured off the mesh) must agree with the kernel's
    /// authoritative mass-properties query within faceting tolerance. If they
    /// ever diverge, the eye is lying and this fails. This is the visual⇄numeric
    /// contract that makes the overlay trustworthy.
    #[test]
    fn eye3_analytics_match_kernel_mass_properties() {
        let mut model = BRepModel::new();
        let s = housing(&mut model);
        let frame = render_dimensioned_multiview(&model, s, &TessellationParams::default())
            .expect("render");
        let mp = model.mass_properties_for(s).expect("mass properties");

        let vrel = (frame.volume - mp.volume).abs() / mp.volume;
        assert!(
            vrel < 0.02,
            "overlay volume {} vs kernel {} (rel {vrel})",
            frame.volume,
            mp.volume
        );
        let arel = (frame.surface_area - mp.surface_area).abs() / mp.surface_area;
        assert!(
            arel < 0.05,
            "overlay area {} vs kernel {} (rel {arel})",
            frame.surface_area,
            mp.surface_area
        );
        let com = mp.center_of_mass;
        let dc = ((frame.centroid.x - com[0]).powi(2)
            + (frame.centroid.y - com[1]).powi(2)
            + (frame.centroid.z - com[2]).powi(2))
        .sqrt();
        assert!(
            dc < 1.0,
            "overlay centroid {:?} vs kernel {:?} (dist {dc})",
            frame.centroid,
            com
        );
    }

    /// EYE-5: the coverage report must HONESTLY partition faces into seen /
    /// unseen across the 4 standard views. A box has 6 faces; iso/front/top/
    /// right only cover one hemisphere, so the 3 back faces (−X/−Y/−Z) must be
    /// reported unseen — the agent must learn it has NOT seen them, not assume
    /// full coverage.
    #[test]
    fn coverage_report_flags_unseen_back_faces() {
        let mut model = BRepModel::new();
        let bx = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(40.0, 40.0, 40.0)
            .expect("box"));
        let r = coverage_report(&model, bx, &TessellationParams::default()).expect("coverage");

        assert_eq!(r.total_faces, 6, "a box has 6 faces");
        // Partition is exact and disjoint.
        assert_eq!(
            r.seen_faces.len() + r.unseen_faces.len(),
            r.total_faces,
            "seen+unseen must equal total"
        );
        for f in &r.seen_faces {
            assert!(!r.unseen_faces.contains(f), "face {f} both seen and unseen");
        }
        // The 4 standard views cover one hemisphere → some faces seen, some not.
        assert!(r.seen_faces.len() >= 3, "should see ≥3 front faces");
        assert!(
            !r.unseen_faces.is_empty(),
            "the box's back faces must be flagged unseen (not silently assumed)"
        );
        assert!(r.coverage_fraction < 1.0, "coverage must not claim 100%");
        assert!(
            r.notes.iter().any(|n| n.contains("request another")),
            "an unseen-faces note must tell the agent to request another angle"
        );
    }

    /// A 60×60×20 plate with a central Ø20 through-bore — a clean solid whose
    /// mid-plane section has a known analytic area.
    fn bored_plate(model: &mut BRepModel) -> SolidId {
        let plate = sid(TopologyBuilder::new(model)
            .create_box_3d(60.0, 60.0, 20.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -20.0),
                V3::new(0.0, 0.0, 1.0),
                10.0,
                80.0,
            )
            .expect("bore"));
        boolean_operation(
            model,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("through bore")
    }

    /// DIAG for the EYE-2 section finding: dump cap count + per-cap area for a
    /// plain box, a plain cylinder, and the bored plate, to localize whether the
    /// section op mishandles the outer boundary, holes, or both.
    #[test]
    fn diag_section_caps() {
        use crate::operations::section::section_solid_by_plane;
        let tri_area = |c: &crate::operations::section::SectionCap| -> f64 {
            c.indices
                .iter()
                .map(|t| {
                    let a = &c.vertices[t[0] as usize];
                    let b = &c.vertices[t[1] as usize];
                    let d = &c.vertices[t[2] as usize];
                    let e1 = V3::new(b.x - a.x, b.y - a.y, b.z - a.z);
                    let e2 = V3::new(d.x - a.x, d.y - a.y, d.z - a.z);
                    e1.cross(&e2).magnitude() * 0.5
                })
                .sum()
        };
        let cases: Vec<(&str, Box<dyn Fn(&mut BRepModel) -> SolidId>)> = vec![
            (
                "plain-box",
                Box::new(|m: &mut BRepModel| {
                    sid(TopologyBuilder::new(m)
                        .create_box_3d(60.0, 60.0, 20.0)
                        .expect("box"))
                }),
            ),
            (
                "plain-cyl",
                Box::new(|m: &mut BRepModel| {
                    sid(TopologyBuilder::new(m)
                        .create_cylinder_3d(
                            Point3::new(0.0, 0.0, -10.0),
                            V3::new(0.0, 0.0, 1.0),
                            10.0,
                            20.0,
                        )
                        .expect("cyl"))
                }),
            ),
            ("bored-plate", Box::new(|m: &mut BRepModel| bored_plate(m))),
        ];
        for (name, build) in &cases {
            let mut m = BRepModel::new();
            let s = build(&mut m);
            let caps = section_solid_by_plane(
                &m,
                s,
                Point3::new(0.0, 0.0, 0.0),
                V3::new(0.0, 0.0, 1.0),
                Tolerance::default(),
            )
            .expect("section");
            eprintln!("{name}: {} caps", caps.len());
            for (i, c) in caps.iter().enumerate() {
                eprintln!(
                    "  cap{i}: verts={} tris={} area={:.2}",
                    c.vertices.len(),
                    c.indices.len(),
                    tri_area(c)
                );
            }
        }
    }

    /// EYE-2 GREEN GUARD: the cross-section area measured off the kernel's cap
    /// triangulation must match analytic. Mid-plane (z=0, +Z) section of a
    /// cylinder (r10, z∈[−10,10]) is a Ø20 disk = 100π ≈ 314.16 mm². (Planar
    /// faces are broken — see #83 — so the guard uses a curved section, which
    /// works correctly.)
    #[test]
    fn eye2_section_area_matches_analytic() {
        let mut model = BRepModel::new();
        let cyl = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -10.0),
                V3::new(0.0, 0.0, 1.0),
                10.0,
                20.0,
            )
            .expect("cyl"));
        let f = render_section(
            &model,
            cyl,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Tolerance::default(),
        )
        .expect("section");

        let expected = std::f64::consts::PI * 100.0;
        let rel = (f.section_area - expected).abs() / expected;
        assert!(
            rel < 0.03,
            "section area {} vs analytic {expected} (rel {rel})",
            f.section_area
        );
        assert!(
            (f.extent_u - 20.0).abs() < 0.5 && (f.extent_v - 20.0).abs() < 0.5,
            "section extents {}×{} expected 20×20",
            f.extent_u,
            f.extent_v
        );
        // Recoverability: the section plane origin projects inside the image.
        let (px, py) = f.project(&f.plane_origin);
        assert!(px >= 0.0 && px < f.width as f64 && py >= 0.0 && py < f.height as f64);
    }

    /// #83 GUARD (fixed): sectioning a solid with PLANAR faces must capture
    /// them. Mid-plane section of the bored plate is the 60×60 square MINUS the
    /// Ø20 bore disk = 3600 − 100π ≈ 3285.8 mm² (planar sides via the exact
    /// Plane×Plane clip + the bore circle via the curved-face marching path).
    #[test]
    fn section_planar_faces_covered_83() {
        let mut model = BRepModel::new();
        let s = bored_plate(&mut model);
        let f = render_section(
            &model,
            s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Tolerance::default(),
        )
        .expect("planar section must produce caps");
        let expected = 60.0 * 60.0 - std::f64::consts::PI * 100.0;
        let rel = (f.section_area - expected).abs() / expected;
        assert!(
            rel < 0.03,
            "section area {} vs analytic {expected}",
            f.section_area
        );
    }

    #[test]
    #[ignore = "writes a PNG for manual inspection"]
    fn emit_section_png() {
        // Bored plate mid-section: a 60×60 square with a Ø20 round hole — the
        // visual proof of the #83 planar-face section fix.
        let mut model = BRepModel::new();
        let part = bored_plate(&mut model);
        let f = render_section(
            &model,
            part,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Tolerance::default(),
        )
        .expect("section");
        std::fs::write("../_section.png", f.to_png().expect("png")).expect("write");
        eprintln!(
            "wrote ../_section.png area={:.1} extent {:.0}x{:.0}",
            f.section_area, f.extent_u, f.extent_v
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
