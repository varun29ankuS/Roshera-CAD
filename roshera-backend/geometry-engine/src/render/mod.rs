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

use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::mesh::TriangleMesh;
use crate::tessellation::{tessellate_solid, TessellationParams};

/// EYE-1: coordinate-anchored dimensioned multi-view render.
pub mod dimensioned;

/// EYE-PROFILE: dimensioned axial-profile (meridian) drawing for axisymmetric
/// solids (nozzles / revolved / lofted bodies).
pub mod profile;

/// EYE-6: active-perception viewpoint selection (next-best-view).
pub mod viewpoint;

/// EYE-SKETCH: the agent eye for 2D sketches — rasterize a sketch to a PNG so a
/// vision-capable agent can SEE it (precondition for semantic recognition).
pub mod sketch;

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

    /// Orthonormal SCREEN basis `(right, up)` for this canonical view — the
    /// same basis [`camera_basis`] derives from `direction()`/`up_hint()`.
    /// Returns `None` only for a degenerate (parallel) pair, which the
    /// canonical directions never produce.
    pub(crate) fn camera_basis(self) -> Option<(Vector3, Vector3)> {
        camera_basis(self.direction(), self.up_hint())
    }
}

/// Orthonormal SCREEN basis `(right, up)` in world space for a camera looking
/// along `dir` (camera→scene) with world-space `up_hint`: `right` → +pixel-x,
/// `up` → +pixel-top (the rasterizer flips v so +up is the top image row).
/// Returns `None` when `up_hint ∥ dir` (a pole degeneracy the caller resolves).
///
/// THE single source of truth for the render camera basis. [`render_mesh_dir`]
/// rasterizes through exactly this basis, and the drawing HLR vector projection
/// (`crate::drawing::projection::view_matrix_for_projection`, `Isometric` arm)
/// derives its page axes from this same function. Sharing one definition is
/// what keeps the isometric cell's shaded-solid raster and its HLR wireframe
/// overlay in ONE pose: they can no longer disagree on iso orientation because
/// there is only one iso camera. Standard engineering isometric is the
/// `CanonicalView::Isometric` case — camera at (1,1,1) (az +45°, el +35.264°),
/// world-Z up — giving `right = (1,−1,0)/√2`, `up = (−1,−1,2)/√6`.
pub(crate) fn camera_basis(dir: Vector3, up_hint: Vector3) -> Option<(Vector3, Vector3)> {
    let right = up_hint.cross(&dir).normalize().ok()?;
    let up = dir.cross(&right).normalize().ok()?;
    Some((right, up))
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

/// Fraction of the framebuffer the auto-framed content fills (the remaining
/// margin splits evenly per side — see the window fit in `render_mesh_dir_marks`).
/// Public because the drawing SVG renderer INVERTS this framing to register the
/// shaded isometric raster with the vector outline overlay drawn on top of it;
/// if this factor changes, that registration follows automatically.
pub const RASTER_FILL_FACTOR: f64 = 0.9;

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

/// Rasterize a raw [`TriangleMesh`] to a [`RenderFrame`] from the canonical
/// view direction in `opts.view` (auto-framed). Back-face culling is OFF in
/// [`RenderMode::Shaded`] — so a flipped-normal defect still produces a solid-
/// looking render, which is exactly what makes it invisible to a VLM while the
/// cert catches it analytically.
///
/// This exposes the core rasterizer to callers that already hold a mesh (e.g.
/// the injected-defect benchmark that mutates a tessellated mesh and renders it
/// WITHOUT going through a B-Rep solid). `opts.tessellation` is ignored here
/// (the caller owns the mesh); all other fields of `opts` apply as usual.
///
/// Returns `None` if `mesh` has no triangles.
pub fn render_mesh(mesh: &TriangleMesh, opts: &RenderOptions) -> Option<RenderFrame> {
    let dir = opts.view.direction();
    let up_hint = opts.view.up_hint();
    render_mesh_dir(mesh, dir, up_hint, opts, None, &[])
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

/// Render one solid with an EXPLICIT DIRECTIONAL KEY LIGHT instead of the
/// default headlight lambert.
///
/// The default `Shaded` shading is a headlight (`|n·view|`), which collapses
/// an isometric cube to ONE uniform gray: all three visible faces make the
/// same angle with the view direction (|n·dir| = 1/√3). For presentation
/// renders (the engineering-drawing pictorial) that reads as a flat blob, so
/// this entry point shades with `0.25 + 0.75·|n·key_light|` — an ambient
/// floor plus a lambert against `key_light` (unit vector pointing TOWARD the
/// light). Choose a light oblique to all principal face normals and the three
/// face families of an iso cube read as three clearly distinct values.
///
/// DELIBERATELY a separate entry point: `render_solid` / `render_mesh` remain
/// byte-identical for every existing consumer (perception gates, defect
/// benchmarks, snapshot diffs). Deterministic: pure function of the mesh and
/// the fixed light, no time-based seeds.
pub fn render_solid_lit(
    model: &BRepModel,
    solid_id: SolidId,
    key_light: Vector3,
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    let solid = model.solids.get(solid_id)?;
    let mesh = tessellate_solid(solid, model, &opts.tessellation);
    render_mesh_dir_marks(
        &mesh,
        opts.view.direction(),
        opts.view.up_hint(),
        opts,
        None,
        &[],
        Some(key_light),
    )
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
    render_mesh_dir(&mesh, dir, up_hint, opts, None, &[])
}

/// One overlaid label callout: a WORLD anchor point and an (ASCII-safe) name.
/// The anchor is projected through the SAME camera the render uses, so the
/// drawn callout always lands on the feature it names — that is what makes the
/// label VERIFIABLE on the picture (you SEE "throat" on the throat face).
pub type LabelCallout = (Point3, String);

/// A COLOR-CODED label mark for the set-of-marks overlay: a world `anchor`, the
/// callout `text` (drawn verbatim — `"name — Ø2.00 mm"`), the label's display
/// `color`, and the optional `target_face` to TINT that same colour so the human
/// / agent can see which mark belongs to which feature. The richer companion to
/// [`LabelCallout`]; [`render_solid_with_label_marks`] draws each callout in its
/// own colour and tints the target face.
#[derive(Debug, Clone)]
pub struct LabelMark {
    pub anchor: Point3,
    pub text: String,
    pub color: [u8; 3],
    pub target_face: Option<crate::primitives::face::FaceId>,
}

/// Render one solid, TINTING each labelled target face in its label colour, then
/// overlay each callout in that same colour with its measurement text. This is
/// the COLOR-CODED set-of-marks eye (named, dimensioned, color-coded callouts):
/// the picture itself shows which mark maps to which feature. Empty `marks` is
/// identical to [`render_solid`].
pub fn render_solid_with_label_marks(
    model: &BRepModel,
    solid_id: SolidId,
    marks: &[LabelMark],
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    let solid = model.solids.get(solid_id)?;
    let mesh = tessellate_solid(solid, model, &opts.tessellation);
    // Per-triangle base colour: a face tinted by the label whose target it is,
    // else the default clay. A face claimed by two labels takes the first (marks
    // are name-ordered, so this is deterministic).
    const DEFAULT: [u8; 3] = [200, 200, 200];
    let mut tri_colors: Vec<[u8; 3]> = Vec::with_capacity(mesh.triangles.len());
    for ti in 0..mesh.triangles.len() {
        let fid = mesh.face_map.get(ti).copied();
        let col = fid
            .and_then(|f| {
                marks
                    .iter()
                    .find(|m| m.target_face == Some(f))
                    .map(|m| m.color)
            })
            .unwrap_or(DEFAULT);
        tri_colors.push(col);
    }
    let callouts: Vec<ColoredCallout> = marks
        .iter()
        .map(|m| (m.anchor, m.text.clone(), m.color))
        .collect();
    render_mesh_dir_marks(
        &mesh,
        opts.view.direction(),
        opts.view.up_hint(),
        opts,
        Some(&tri_colors),
        &callouts,
        None,
    )
}

/// A coloured callout for the set-of-marks overlay: anchor, text, RGB.
type ColoredCallout = (Point3, String, [u8; 3]);

/// Render one solid through the canonical view of `opts`, then overlay the
/// given label callouts (leader line from the projected anchor to a text tag).
/// The LABELLER's eye: the agent and the user see the named features marked on
/// the part. Empty `callouts` is identical to [`render_solid`].
pub fn render_solid_with_labels(
    model: &BRepModel,
    solid_id: SolidId,
    callouts: &[LabelCallout],
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    let solid = model.solids.get(solid_id)?;
    let mesh = tessellate_solid(solid, model, &opts.tessellation);
    render_mesh_dir(
        &mesh,
        opts.view.direction(),
        opts.view.up_hint(),
        opts,
        None,
        callouts,
    )
}

/// Scene render — composite EVERY solid in `solid_ids` into one frame from an
/// arbitrary view direction (auto-framed to the combined bounds). This is the
/// agent's eye on a whole ASSEMBLY (not just one part): the per-solid meshes are
/// tessellated and merged into a single mesh, then rasterized through the same
/// path as `render_solid_dir`. `dir` points camera→scene; `up_hint` must not be
/// parallel to it.
/// `colors`, when non-empty, gives a per-solid base RGB parallel to `solid_ids`
/// (missing/short entries fall back to light grey) — the Shaded pass tints each
/// solid by it, so the agent's eye sees a coloured assembly (black tyres, livery
/// body, …) instead of monochrome clay.
pub fn render_solids_dir(
    model: &BRepModel,
    solid_ids: &[SolidId],
    colors: &[[u8; 3]],
    dir: Vector3,
    up_hint: Vector3,
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    const DEFAULT: [u8; 3] = [200, 200, 200];
    let mut merged = TriangleMesh::new();
    let mut tri_colors: Vec<[u8; 3]> = Vec::new();
    for (si, &id) in solid_ids.iter().enumerate() {
        let solid = match model.solids.get(id) {
            Some(s) => s,
            None => continue,
        };
        let col = colors.get(si).copied().unwrap_or(DEFAULT);
        let m = tessellate_solid(solid, model, &opts.tessellation);
        let base = merged.vertices.len() as u32;
        merged.vertices.extend_from_slice(&m.vertices);
        for (ti, t) in m.triangles.iter().enumerate() {
            merged
                .triangles
                .push([t[0] + base, t[1] + base, t[2] + base]);
            merged
                .face_map
                .push(m.face_map.get(ti).copied().unwrap_or(u32::MAX));
            tri_colors.push(col);
        }
    }
    render_mesh_dir(&merged, dir, up_hint, opts, Some(&tri_colors), &[])
}

/// INSTANCED scene render — the true-assembly eye. Each instance is a
/// `(solid_id, world_transform, rgb)` triple: the SAME solid id may appear
/// in many instances at different transforms (geometry is referenced, never
/// copied). For each instance the solid's mesh is tessellated ONCE per
/// distinct solid would be ideal, but tessellation is cheap relative to the
/// rest of the pipeline and the per-instance transform must be baked into
/// world space anyway, so we tessellate-then-transform per instance. The
/// instance transform is applied to vertex positions (full affine) and to
/// normals (rotation/linear part only — translation does not move a
/// direction). `dir` points camera→scene; `up_hint` must not be parallel.
///
/// This is what makes a 100-part assembly tractable: the kernel stores N
/// instances referencing M ≤ N parts, and only the render walks the
/// instance list. Returns `None` if every instance resolves to a missing or
/// empty solid.
pub fn render_instances_dir(
    model: &BRepModel,
    instances: &[(SolidId, Matrix4, [u8; 3])],
    dir: Vector3,
    up_hint: Vector3,
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    let mut merged = TriangleMesh::new();
    let mut tri_colors: Vec<[u8; 3]> = Vec::new();
    for (solid_id, transform, color) in instances {
        let solid = match model.solids.get(*solid_id) {
            Some(s) => s,
            None => continue,
        };
        let m = tessellate_solid(solid, model, &opts.tessellation);
        let base = merged.vertices.len() as u32;
        for v in &m.vertices {
            let position = transform.transform_point(&v.position);
            // Normal: linear part only. Re-normalise in case the transform
            // carries a uniform scale; a non-uniform scale would skew the
            // normal but instancing transforms are rigid in Phase 1.
            let n = transform.transform_vector(&v.normal);
            let normal = n.normalize().unwrap_or(v.normal);
            merged.vertices.push(crate::tessellation::mesh::MeshVertex {
                position,
                normal,
                uv: v.uv,
            });
        }
        for (ti, t) in m.triangles.iter().enumerate() {
            merged
                .triangles
                .push([t[0] + base, t[1] + base, t[2] + base]);
            merged
                .face_map
                .push(m.face_map.get(ti).copied().unwrap_or(u32::MAX));
            tri_colors.push(*color);
        }
    }
    render_mesh_dir(&merged, dir, up_hint, opts, Some(&tri_colors), &[])
}

/// Scene render with label callouts overlaid — the assembly variant of
/// [`render_solid_with_labels`]. Composites every solid (tinted by `colors`),
/// then draws the named-feature callouts through the shared camera.
#[allow(clippy::too_many_arguments)]
pub fn render_solids_with_labels(
    model: &BRepModel,
    solid_ids: &[SolidId],
    colors: &[[u8; 3]],
    callouts: &[LabelCallout],
    dir: Vector3,
    up_hint: Vector3,
    opts: &RenderOptions,
) -> Option<RenderFrame> {
    const DEFAULT: [u8; 3] = [200, 200, 200];
    let mut merged = TriangleMesh::new();
    let mut tri_colors: Vec<[u8; 3]> = Vec::new();
    for (si, &id) in solid_ids.iter().enumerate() {
        let solid = match model.solids.get(id) {
            Some(s) => s,
            None => continue,
        };
        let col = colors.get(si).copied().unwrap_or(DEFAULT);
        let m = tessellate_solid(solid, model, &opts.tessellation);
        let base = merged.vertices.len() as u32;
        merged.vertices.extend_from_slice(&m.vertices);
        for (ti, t) in m.triangles.iter().enumerate() {
            merged
                .triangles
                .push([t[0] + base, t[1] + base, t[2] + base]);
            merged
                .face_map
                .push(m.face_map.get(ti).copied().unwrap_or(u32::MAX));
            tri_colors.push(col);
        }
    }
    render_mesh_dir(&merged, dir, up_hint, opts, Some(&tri_colors), callouts)
}

/// Rasterize an already-tessellated mesh from a view direction (auto-framed).
/// Shared by `render_solid_dir` (one solid) and `render_solids_dir` (assembly).
/// Thin wrapper: rasterize with MONO-coloured callouts (the default labeller
/// blue). Keeps the original `render_mesh_dir` contract for callers that do not
/// carry per-label colour.
#[allow(clippy::too_many_arguments)]
fn render_mesh_dir(
    mesh: &TriangleMesh,
    dir: Vector3,
    up_hint: Vector3,
    opts: &RenderOptions,
    tri_colors: Option<&[[u8; 3]]>,
    label_callouts: &[LabelCallout],
) -> Option<RenderFrame> {
    const LABEL_COLOR: [u8; 3] = [0, 90, 200]; // blue — distinct from dim-orange
    let colored: Vec<ColoredCallout> = label_callouts
        .iter()
        .map(|(p, s)| (*p, s.clone(), LABEL_COLOR))
        .collect();
    render_mesh_dir_marks(mesh, dir, up_hint, opts, tri_colors, &colored, None)
}

/// Rasterize an already-tessellated mesh, then overlay COLOUR-CODED callouts —
/// each callout drawn in its own colour (the set-of-marks eye). Shared core of
/// every label-overlay render path.
///
/// `key_light`: `None` = headlight shading (`0.26 + 0.74·|n·view|`, the
/// long-standing default — byte-identical for all existing consumers).
/// `Some(l)` = directional key light (`0.25 + 0.75·|n·l|`), used by
/// [`render_solid_lit`] for presentation renders where distinct face values
/// matter. Applies to [`RenderMode::Shaded`] only.
#[allow(clippy::too_many_arguments)]
fn render_mesh_dir_marks(
    mesh: &TriangleMesh,
    dir: Vector3,
    up_hint: Vector3,
    opts: &RenderOptions,
    tri_colors: Option<&[[u8; 3]]>,
    label_callouts: &[ColoredCallout],
    key_light: Option<Vector3>,
) -> Option<RenderFrame> {
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

    // Camera basis: right/up orthonormal to the view direction. Single source
    // of truth (also consumed by the drawing HLR iso projection so the shaded
    // raster and the wireframe overlay share one pose) — see `camera_basis`.
    let (right, up) = match camera_basis(dir, up_hint) {
        Some(b) => b,
        None => return None,
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
    let scale = ((opts.width as f64 * RASTER_FILL_FACTOR) / span_u)
        .min((opts.height as f64 * RASTER_FILL_FACTOR) / span_v);
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
                // Lambert, orientation-independent via abs(), TINTING a
                // per-triangle base colour (grey when none supplied). Default
                // is the headlight (light = view direction; byte-identical to
                // the historical output); a caller-supplied key light shades
                // against that direction instead with a 0.25 ambient floor so
                // oblique face families read as distinct values.
                let f = match key_light {
                    Some(l) => 0.25 + 0.75 * tri_normal.dot(&l).abs(),
                    None => 0.26 + 0.74 * tri_normal.dot(&dir).abs(),
                };
                let base = tri_colors
                    .and_then(|c| c.get(ti).copied())
                    .unwrap_or([200, 200, 200]);
                [
                    (base[0] as f64 * f) as u8,
                    (base[1] as f64 * f) as u8,
                    (base[2] as f64 * f) as u8,
                ]
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

    // ── Label callout overlay (the LABELLER's eye) ───────────────────────────
    // Project each world anchor through the SAME orthographic camera the mesh
    // used (so the callout always lands on the feature it names), then draw a
    // crosshair tick at the anchor, a leader line to a text tag, and the name.
    // Greedy vertical de-overlap keeps stacked names legible. Reuses the 5×7
    // font + Bresenham line shared from `dimensioned`, so the labeller overlay
    // and the dimension overlay are visually identical and share one font.
    if !label_callouts.is_empty() {
        const SCALE: usize = 2;
        let (w, h) = (opts.width, opts.height);
        let th = crate::render::dimensioned::glyph_height_px(SCALE);
        // Project an arbitrary world point through the active camera + window.
        let project = |p: &Point3| -> (f64, f64, f64) {
            let q = Vector3::new(p.x, p.y, p.z);
            let (u, v, ww) = (q.dot(&right), q.dot(&up), q.dot(&dir));
            (
                u * scale + off_u,
                opts.height as f64 - (v * scale + off_v),
                ww,
            )
        };
        let mut placed: Vec<(f64, f64, f64, f64)> = Vec::new();
        for (anchor, name, label_color) in label_callouts {
            let label_color = *label_color;
            let (ax, ay, _depth) = project(anchor);
            // Skip anchors that fall outside the framebuffer.
            if ax < 0.0 || ay < 0.0 || ax >= w as f64 || ay >= h as f64 {
                continue;
            }
            let tw = crate::render::dimensioned::text_width_px(name, SCALE);
            // Default label box up-right of the anchor; flip to stay in-bounds.
            let mut lx = ax + 8.0;
            let mut ly = ay - 8.0 - th;
            if lx + tw > w as f64 - 2.0 {
                lx = (ax - 8.0 - tw).max(2.0);
            }
            if ly < 2.0 {
                ly = ay + 8.0;
            }
            // Greedy vertical de-overlap against already-placed labels.
            let mut guard = 0;
            loop {
                let b = (lx, ly, lx + tw, ly + th);
                let hit = placed
                    .iter()
                    .any(|q| !(b.2 < q.0 || b.0 > q.2 || b.3 < q.1 || b.1 > q.3));
                if !hit || guard > 40 || ly + 2.0 * th > h as f64 - 2.0 {
                    break;
                }
                ly += th + 2.0;
                guard += 1;
            }
            placed.push((lx, ly, lx + tw, ly + th));
            // Anchor crosshair, leader to the label box, then the name.
            crate::render::dimensioned::draw_line_overlay(
                &mut pixels,
                w,
                h,
                ax - 4.0,
                ay,
                ax + 4.0,
                ay,
                label_color,
            );
            crate::render::dimensioned::draw_line_overlay(
                &mut pixels,
                w,
                h,
                ax,
                ay - 4.0,
                ax,
                ay + 4.0,
                label_color,
            );
            crate::render::dimensioned::draw_line_overlay(
                &mut pixels,
                w,
                h,
                ax,
                ay,
                lx,
                ly + th,
                label_color,
            );
            crate::render::dimensioned::draw_text_overlay(
                &mut pixels,
                w,
                h,
                lx,
                ly,
                name,
                label_color,
                SCALE,
            );
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
