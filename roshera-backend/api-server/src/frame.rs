//! `GET /api/frame` — server-rendered PNG of the current scene.
//!
//! This is the kernel's *exteroception* sense: a multimodal LLM (or
//! anyone debugging from `curl`) can now ask "what does the model
//! look like right now?" and get back an image, no GPU or browser
//! involved. The pipeline is intentionally pure-CPU so the binary
//! deploys anywhere `cargo run` does.
//!
//! ## Pipeline
//!
//! 1. Read-lock the live `BRepModel`.
//! 2. Tessellate every solid (`tessellate_solid`) into a list of
//!    triangles in world space, with per-triangle face normals.
//! 3. Compute the scene bounding box and, unless the caller supplied
//!    explicit camera parameters, fit an isometric camera around it.
//! 4. Project every triangle through view + perspective and rasterize
//!    with a Z-buffer + Lambert shading on the CPU.
//! 5. Encode the framebuffer as a PNG and stream it back as
//!    `image/png`.
//!
//! ## Supported query parameters
//!
//! All optional. Sensible defaults render an isometric 512×512 view
//! that auto-fits the scene's bounding box.
//!
//! | Parameter   | Default      | Description |
//! |-------------|--------------|-------------|
//! | `width`     | 512          | Output image width (pixels, 1..=2048). |
//! | `height`    | 512          | Output image height (pixels, 1..=2048). |
//! | `view`      | `iso`        | One of `iso`, `front`, `back`, `top`, `bottom`, `right`, `left`. Ignored if `eye_x`/`eye_y`/`eye_z` are provided. |
//! | `eye_x,y,z` | (auto)       | World-space camera position. All three required to override. |
//! | `target_x,y,z` | scene centre | World-space look-at point. |
//! | `fov_deg`   | 35           | Vertical field of view in degrees (5..=120). |
//!
//! ## Why CPU rasterization
//!
//! The alternatives were OpenGL, wgpu, or a third-party rasterizer.
//! All of those add a runtime dependency we'd have to manage on every
//! deployment surface (Linux server, Windows desktop, Docker). A few
//! hundred lines of triangle rasterizer here trade a small amount of
//! per-pixel cost for zero deploy friction. At 512×512 the typical
//! scene renders well below 100 ms — fast enough for an agent's
//! observe-act loop.
//!
//! ## Lighting
//!
//! Single directional light with ambient. Each triangle receives
//! `ambient + diffuse · max(0, n · l)` modulated by a per-solid
//! pseudo-random hue so distinct solids are visually separable. The
//! background is the blueprint off-white (#f4eedb) the frontend
//! viewer uses, so a screenshot looks at home next to a live
//! viewport.

#![allow(clippy::indexing_slicing)] // Framebuffer + zbuffer access is bounds-checked by (w*h) allocation.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde::Deserialize;
use std::io::Cursor;

/// Hard upper bound on output dimensions. 2048×2048 RGBA is ~16 MB —
/// large enough for any agent screenshot, small enough that a
/// pathological caller can't OOM the server.
const MAX_DIM: u32 = 2048;

/// Background colour, RGB. Matches the blueprint canvas the frontend
/// viewport draws on so a screenshot is visually consistent with the
/// live UI.
const BG_RGB: [u8; 3] = [0xf4, 0xee, 0xdb];

/// Ambient term in the Lambert shader (0..1). Stops back-facing
/// triangles from going pure-black on a single-light scene.
const AMBIENT: f64 = 0.25;

/// Direction the key light comes *from*, in world space, before
/// normalisation. Picked to give every face of an axis-aligned box a
/// distinguishable shade.
const LIGHT_DIR_RAW: [f64; 3] = [-0.4, -0.6, -0.7];

/// Query parameters accepted by `GET /api/frame`. Each field is
/// individually optional; missing fields fall back to the auto-fit
/// camera + 512×512 defaults documented at the module level.
#[derive(Debug, Default, Deserialize)]
pub struct FrameQuery {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub view: Option<String>,
    pub eye_x: Option<f64>,
    pub eye_y: Option<f64>,
    pub eye_z: Option<f64>,
    pub target_x: Option<f64>,
    pub target_y: Option<f64>,
    pub target_z: Option<f64>,
    pub fov_deg: Option<f64>,
}

/// Camera resolved from the query (or auto-fit when nothing is
/// supplied). Fields are world-space.
#[derive(Debug, Clone, Copy)]
struct Camera {
    eye: Point3,
    target: Point3,
    up: Vector3,
    fovy_deg: f64,
    znear: f64,
    zfar: f64,
}

/// Axis-aligned scene bounding box, plus a flag for "no geometry at all".
#[derive(Debug, Clone, Copy)]
struct SceneBounds {
    min: Point3,
    max: Point3,
    has_geometry: bool,
}

impl SceneBounds {
    fn empty() -> Self {
        Self {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(0.0, 0.0, 0.0),
            has_geometry: false,
        }
    }

    fn extend(&mut self, p: Point3) {
        if !self.has_geometry {
            self.min = p;
            self.max = p;
            self.has_geometry = true;
            return;
        }
        if p.x < self.min.x {
            self.min.x = p.x;
        }
        if p.y < self.min.y {
            self.min.y = p.y;
        }
        if p.z < self.min.z {
            self.min.z = p.z;
        }
        if p.x > self.max.x {
            self.max.x = p.x;
        }
        if p.y > self.max.y {
            self.max.y = p.y;
        }
        if p.z > self.max.z {
            self.max.z = p.z;
        }
    }

    fn center(&self) -> Point3 {
        Point3::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
            (self.min.z + self.max.z) * 0.5,
        )
    }

    /// Bounding-sphere radius. Always strictly positive — a degenerate
    /// scene (single point or empty) is reported as radius 1 so the
    /// auto camera still produces a viewable image.
    fn radius(&self) -> f64 {
        if !self.has_geometry {
            return 1.0;
        }
        let dx = self.max.x - self.min.x;
        let dy = self.max.y - self.min.y;
        let dz = self.max.z - self.min.z;
        let r = 0.5 * (dx * dx + dy * dy + dz * dz).sqrt();
        if r < 1e-9 {
            1.0
        } else {
            r
        }
    }
}

/// One rendered triangle — three world-space vertices, the face
/// normal already normalised, and a base RGB colour.
#[derive(Debug, Clone, Copy)]
struct WorldTri {
    v: [Point3; 3],
    normal: Vector3,
    color: [u8; 3],
}

/// `GET /api/frame` handler. See the module docs for the full
/// parameter contract.
pub async fn get_frame(
    State(state): State<AppState>,
    Query(q): Query<FrameQuery>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let width = clamp_dim(q.width.unwrap_or(512))?;
    let height = clamp_dim(q.height.unwrap_or(512))?;
    let fov_deg = q.fov_deg.unwrap_or(35.0);
    if !(5.0..=120.0).contains(&fov_deg) {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("fov_deg must be in [5, 120]; got {fov_deg}"),
        )
        .into());
    }

    let model = state.model.read().await;
    let bounds = compute_bounds(&model);
    let camera = resolve_camera(&q, &bounds, width, height, fov_deg);
    let aspect = width as f64 / height as f64;
    let png = render_png(&model, &camera, aspect, width, height);
    drop(model);

    let png = png.map_err(|e| {
        ApiError::new(
            ErrorCode::Internal,
            format!("frame render failed: {e}"),
        )
    })?;

    let mut resp = Response::new(Body::from(png));
    *resp.status_mut() = StatusCode::OK;
    if let Ok(v) = header::HeaderValue::from_str("image/png") {
        resp.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    Ok(resp)
}

fn clamp_dim(d: u32) -> Result<u32, ApiError> {
    if d == 0 || d > MAX_DIM {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("width/height must be in [1, {MAX_DIM}]; got {d}"),
        ));
    }
    Ok(d)
}

/// Walk every solid in the model, tessellating each, and accumulate a
/// world-space bounding box. The model lock must be held by the
/// caller.
fn compute_bounds(model: &BRepModel) -> SceneBounds {
    let mut bounds = SceneBounds::empty();
    let params = TessellationParams::default();
    for (_, solid) in model.solids.iter() {
        let mesh = tessellate_solid(solid, model, &params);
        for v in &mesh.vertices {
            bounds.extend(v.position);
        }
    }
    bounds
}

/// Build the camera. Either the caller supplied a complete `eye_*` /
/// `target_*` triple (we honour it verbatim) or we fit an isometric
/// camera that frames the bounding box.
fn resolve_camera(q: &FrameQuery, b: &SceneBounds, w: u32, h: u32, fov_deg: f64) -> Camera {
    let center = b.center();
    let target = match (q.target_x, q.target_y, q.target_z) {
        (Some(x), Some(y), Some(z)) => Point3::new(x, y, z),
        _ => center,
    };

    let eye = match (q.eye_x, q.eye_y, q.eye_z) {
        (Some(x), Some(y), Some(z)) => Point3::new(x, y, z),
        _ => default_eye(q.view.as_deref(), b, fov_deg, w, h),
    };

    // Up vector: world +Z, except for top/bottom views where Z is
    // collinear with view direction and we fall back to +Y.
    let up = match q.view.as_deref() {
        Some("top") | Some("bottom") => Vector3::new(0.0, 1.0, 0.0),
        _ => Vector3::new(0.0, 0.0, 1.0),
    };

    let r = b.radius();
    Camera {
        eye,
        target,
        up,
        fovy_deg: fov_deg,
        znear: (r * 0.05).max(1e-3),
        zfar: r * 20.0 + 1.0,
    }
}

/// Pick a camera position that frames `bounds` from the requested
/// view. Distance is computed so the bounding sphere fits inside the
/// vertical field of view with ~10 % margin.
fn default_eye(view: Option<&str>, bounds: &SceneBounds, fov_deg: f64, w: u32, h: u32) -> Point3 {
    let r = bounds.radius();
    let half_fov = (fov_deg.to_radians() * 0.5).tan().max(1e-6);
    let aspect = w as f64 / h as f64;
    let half_fov_x = half_fov * aspect;
    let smaller = half_fov.min(half_fov_x);
    let dist = (r / smaller) * 1.1 + r;

    let dir = match view.unwrap_or("iso") {
        "front" => Vector3::new(0.0, -1.0, 0.0),
        "back" => Vector3::new(0.0, 1.0, 0.0),
        "right" => Vector3::new(1.0, 0.0, 0.0),
        "left" => Vector3::new(-1.0, 0.0, 0.0),
        "top" => Vector3::new(0.0, 0.0, 1.0),
        "bottom" => Vector3::new(0.0, 0.0, -1.0),
        // Default isometric direction (1, -1, 1) normalised.
        _ => Vector3::new(1.0, -1.0, 1.0),
    };
    let dir = dir
        .normalize()
        .ok()
        .unwrap_or(Vector3::new(0.577, -0.577, 0.577));

    let c = bounds.center();
    Point3::new(
        c.x + dir.x * dist,
        c.y + dir.y * dist,
        c.z + dir.z * dist,
    )
}

/// Camera basis (right, up, forward) in world space. Used both to
/// project vertices and to transform the light direction into
/// camera-relative coordinates if needed (we keep light in world
/// space here, simpler).
#[derive(Debug, Clone, Copy)]
struct Basis {
    right: Vector3,
    up: Vector3,
    forward: Vector3,
}

fn camera_basis(cam: &Camera) -> Basis {
    let f = (cam.target - cam.eye).normalize().ok().unwrap_or(Vector3::Y);
    let r = f
        .cross(&cam.up)
        .normalize()
        .ok()
        .unwrap_or(Vector3::X);
    let u = r.cross(&f);
    Basis {
        right: r,
        up: u,
        forward: f,
    }
}

/// Project a world-space point to (screen_x, screen_y, depth_along_view).
/// Returns `None` when the point is behind the camera (cz <= znear).
/// Far-plane culling is not performed here: a triangle that straddles
/// the far plane still contributes correctly because the rasterizer
/// only writes pixels where Z is in [0, 1].
fn project(p: Point3, cam: &Camera, basis: &Basis, w: u32, h: u32) -> Option<(f64, f64, f64)> {
    let d = p - cam.eye;
    let cx = d.dot(&basis.right);
    let cy = d.dot(&basis.up);
    let cz = d.dot(&basis.forward);
    if cz < cam.znear {
        return None;
    }
    let half_h = (cam.fovy_deg.to_radians() * 0.5).tan();
    let aspect = w as f64 / h as f64;
    let half_w = half_h * aspect;

    let ndc_x = cx / (cz * half_w);
    let ndc_y = cy / (cz * half_h);

    let sx = (ndc_x + 1.0) * 0.5 * (w as f64);
    // Y axis flips: NDC +Y is up, image +Y is down.
    let sy = (1.0 - (ndc_y + 1.0) * 0.5) * (h as f64);

    let ndc_z = ((cz - cam.znear) / (cam.zfar - cam.znear)).clamp(0.0, 1.0);
    Some((sx, sy, ndc_z))
}

/// Rasterize one triangle into the framebuffer + Z-buffer. Inputs are
/// already in screen space (pixel coordinates) with NDC depth in
/// `[0, 1]`. The depth test is "less" — closer pixels overwrite.
fn rasterize_triangle(
    fb: &mut [u8],
    zb: &mut [f32],
    w: u32,
    h: u32,
    s: [(f64, f64, f64); 3],
    color: [u8; 3],
) {
    let (x0, y0, z0) = s[0];
    let (x1, y1, z1) = s[1];
    let (x2, y2, z2) = s[2];

    let min_x = x0.min(x1).min(x2).floor().max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).ceil().min(w as f64) as i32;
    let min_y = y0.min(y1).min(y2).floor().max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).ceil().min(h as f64) as i32;

    if min_x >= max_x || min_y >= max_y {
        return;
    }

    let denom = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2);
    if denom.abs() < 1e-12 {
        return; // degenerate
    }

    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let w0 = ((y1 - y2) * (px - x2) + (x2 - x1) * (py - y2)) / denom;
            let w1 = ((y2 - y0) * (px - x2) + (x0 - x2) * (py - y2)) / denom;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * z0 + w1 * z1 + w2 * z2;
            let idx = (y as usize) * (w as usize) + (x as usize);
            if (z as f32) < zb[idx] {
                zb[idx] = z as f32;
                let pix = idx * 3;
                fb[pix] = color[0];
                fb[pix + 1] = color[1];
                fb[pix + 2] = color[2];
            }
        }
    }
}

/// Stable per-solid hue. Distinguishes solids in the rendered image
/// without needing material assignments. Pure function of solid id so
/// re-renders are deterministic.
fn solid_color(solid_id: u32) -> [f64; 3] {
    // Hash → hue via golden-ratio multiplier. Saturation and value
    // pinned high enough that every result reads as a solid colour
    // against the off-white background.
    let h = ((solid_id as f64 * 0.6180339887) % 1.0) * 360.0;
    hsv_to_rgb(h, 0.55, 0.92)
}

fn hsv_to_rgb(h_deg: f64, s: f64, v: f64) -> [f64; 3] {
    let c = v * s;
    let h_p = (h_deg / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (h_p.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match h_p as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r1 + m, g1 + m, b1 + m]
}

fn shade(base_rgb: [f64; 3], normal: Vector3) -> [u8; 3] {
    let l = Vector3::new(-LIGHT_DIR_RAW[0], -LIGHT_DIR_RAW[1], -LIGHT_DIR_RAW[2])
        .normalize()
        .ok()
        .unwrap_or(Vector3::Z);
    let ndl = normal.dot(&l).max(0.0);
    let intensity = (AMBIENT + (1.0 - AMBIENT) * ndl).clamp(0.0, 1.0);
    let scale = |c: f64| -> u8 {
        let v = (c * 255.0 * intensity).clamp(0.0, 255.0);
        v as u8
    };
    [scale(base_rgb[0]), scale(base_rgb[1]), scale(base_rgb[2])]
}

/// Top-level pipeline: tessellate every solid, project + rasterize,
/// encode PNG. Returns the encoded PNG bytes ready to ship over HTTP.
fn render_png(
    model: &BRepModel,
    cam: &Camera,
    _aspect: f64,
    w: u32,
    h: u32,
) -> Result<Vec<u8>, String> {
    let basis = camera_basis(cam);
    let pixel_count = (w as usize) * (h as usize);
    let mut fb = vec![0u8; pixel_count * 3];
    for px in 0..pixel_count {
        let pi = px * 3;
        fb[pi] = BG_RGB[0];
        fb[pi + 1] = BG_RGB[1];
        fb[pi + 2] = BG_RGB[2];
    }
    let mut zb = vec![1.0f32; pixel_count];

    let params = TessellationParams::default();
    for (sid, solid) in model.solids.iter() {
        let base_rgb = solid_color(sid);
        let mesh = tessellate_solid(solid, model, &params);
        for tri in &mesh.triangles {
            let i0 = tri[0] as usize;
            let i1 = tri[1] as usize;
            let i2 = tri[2] as usize;
            if i0 >= mesh.vertices.len() || i1 >= mesh.vertices.len() || i2 >= mesh.vertices.len() {
                continue;
            }
            let p0 = mesh.vertices[i0].position;
            let p1 = mesh.vertices[i1].position;
            let p2 = mesh.vertices[i2].position;
            let normal = (p1 - p0)
                .cross(&(p2 - p0))
                .normalize()
                .ok()
                .unwrap_or(Vector3::Z);
            let color = shade(base_rgb, normal);

            let s0 = match project(p0, cam, &basis, w, h) {
                Some(s) => s,
                None => continue,
            };
            let s1 = match project(p1, cam, &basis, w, h) {
                Some(s) => s,
                None => continue,
            };
            let s2 = match project(p2, cam, &basis, w, h) {
                Some(s) => s,
                None => continue,
            };

            rasterize_triangle(&mut fb, &mut zb, w, h, [s0, s1, s2], color);
        }
    }

    encode_png(&fb, w, h)
}

fn encode_png(rgb: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(rgb.len() / 4);
    {
        let cursor = Cursor::new(&mut out);
        let mut encoder = png::Encoder::new(cursor, w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("png header: {e}"))?;
        writer
            .write_image_data(rgb)
            .map_err(|e| format!("png data: {e}"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_dim_rejects_zero() {
        assert!(clamp_dim(0).is_err());
    }

    #[test]
    fn clamp_dim_rejects_oversized() {
        assert!(clamp_dim(MAX_DIM + 1).is_err());
    }

    #[test]
    fn clamp_dim_accepts_typical() {
        assert_eq!(clamp_dim(512).unwrap(), 512);
        assert_eq!(clamp_dim(MAX_DIM).unwrap(), MAX_DIM);
    }

    #[test]
    fn empty_bounds_has_unit_radius() {
        let b = SceneBounds::empty();
        assert_eq!(b.radius(), 1.0);
        assert!(!b.has_geometry);
    }

    #[test]
    fn bounds_extend_tracks_extent() {
        let mut b = SceneBounds::empty();
        b.extend(Point3::new(-1.0, -2.0, -3.0));
        b.extend(Point3::new(4.0, 5.0, 6.0));
        assert!(b.has_geometry);
        assert_eq!(b.center(), Point3::new(1.5, 1.5, 1.5));
        assert!((b.radius() - 0.5 * ((5.0_f64).powi(2) + 7.0_f64.powi(2) + 9.0_f64.powi(2)).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn solid_colors_distinct_for_neighbouring_ids() {
        let c0 = solid_color(0);
        let c1 = solid_color(1);
        let c2 = solid_color(2);
        let dist = |a: [f64; 3], b: [f64; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        assert!(dist(c0, c1) > 0.1, "id 0 and 1 must differ");
        assert!(dist(c1, c2) > 0.1, "id 1 and 2 must differ");
    }

    #[test]
    fn project_returns_none_behind_camera() {
        let cam = Camera {
            eye: Point3::new(0.0, 0.0, 10.0),
            target: Point3::new(0.0, 0.0, 0.0),
            up: Vector3::Z,
            fovy_deg: 35.0,
            znear: 0.1,
            zfar: 100.0,
        };
        let basis = camera_basis(&cam);
        // Point behind camera (further along +Z than eye)
        assert!(project(Point3::new(0.0, 0.0, 20.0), &cam, &basis, 256, 256).is_none());
    }

    #[test]
    fn project_round_trips_origin_to_screen_center() {
        let cam = Camera {
            eye: Point3::new(10.0, 0.0, 0.0),
            target: Point3::new(0.0, 0.0, 0.0),
            up: Vector3::Z,
            fovy_deg: 35.0,
            znear: 0.1,
            zfar: 100.0,
        };
        let basis = camera_basis(&cam);
        let s = project(Point3::new(0.0, 0.0, 0.0), &cam, &basis, 256, 256).unwrap();
        // Centre of a 256×256 image is (128, 128) ± rounding from float math.
        assert!((s.0 - 128.0).abs() < 1.0);
        assert!((s.1 - 128.0).abs() < 1.0);
    }

    #[test]
    fn rasterize_triangle_writes_inside_pixels_only() {
        let w = 16u32;
        let h = 16u32;
        let mut fb = vec![0u8; (w as usize) * (h as usize) * 3];
        let mut zb = vec![1.0f32; (w as usize) * (h as usize)];
        // Right triangle filling the bottom-left quadrant.
        rasterize_triangle(
            &mut fb,
            &mut zb,
            w,
            h,
            [(0.5, 0.5, 0.5), (10.5, 0.5, 0.5), (0.5, 10.5, 0.5)],
            [255, 0, 0],
        );
        // Pixel at (3, 3) should be inside.
        let inside = (3 * 16 + 3) * 3;
        assert_eq!(fb[inside], 255);
        // Pixel at (15, 15) should be untouched (still zero).
        let outside = (15 * 16 + 15) * 3;
        assert_eq!(fb[outside], 0);
    }

    #[test]
    fn empty_model_renders_solid_background() {
        let model = BRepModel::new();
        let cam = Camera {
            eye: Point3::new(10.0, -10.0, 10.0),
            target: Point3::new(0.0, 0.0, 0.0),
            up: Vector3::Z,
            fovy_deg: 35.0,
            znear: 0.1,
            zfar: 100.0,
        };
        let png = render_png(&model, &cam, 1.0, 32, 32).unwrap();
        // PNG signature
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }
}
