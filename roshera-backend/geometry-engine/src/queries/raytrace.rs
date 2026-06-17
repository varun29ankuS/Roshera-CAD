//! Analytic orthographic raytrace (#12 slice 2) — the eye's G-buffer.
//!
//! One EXACT ray per pixel through `raycast_solid`, producing per-pixel depth /
//! oriented normal / FACE ID. Unlike a tessellation render (which can paper over
//! a broken B-Rep into a plausible-looking mesh), this G-buffer is sound: every
//! lit pixel is a real ray↔analytic-surface hit recoverable to `(face,
//! world-xyz, normal)`, and a MISSING face produces no hit — the eye sees
//! THROUGH the hole to whatever is behind, so defects reveal themselves instead
//! of being masked.

use super::raycast::raycast_solid;
use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Per-pixel analytic G-buffer. Row-major, `width * height`. Miss pixels have
/// `hit=false`, `face_id=0`, `depth=+inf`, `normal=[0,0,0]`.
#[derive(Debug, Clone)]
pub struct RaytraceFrame {
    pub width: usize,
    pub height: usize,
    pub hit: Vec<bool>,
    pub face_id: Vec<u32>,
    pub depth: Vec<f64>,
    pub normal: Vec<[f64; 3]>,
}

impl RaytraceFrame {
    /// Index of pixel `(px, py)` (row-major, y-down).
    #[inline]
    pub fn idx(&self, px: usize, py: usize) -> usize {
        py * self.width + px
    }
    /// Fraction of pixels that hit the solid.
    pub fn coverage(&self) -> f64 {
        let n = self.hit.iter().filter(|&&h| h).count();
        n as f64 / (self.width * self.height).max(1) as f64
    }
    /// Distinct face ids visible in the frame.
    pub fn visible_faces(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .face_id
            .iter()
            .zip(self.hit.iter())
            .filter(|(_, &h)| h)
            .map(|(&f, _)| f)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// Orthographic analytic raytrace.
///
/// `center` is the world point the frame is centred on; `right`/`up` span the
/// image plane; `dir` is the view direction (rays travel along +dir). `half_w`
/// is half the frame width in world units (height follows the pixel aspect).
/// Each ray starts `back` world-units behind `center` along `-dir`, so the
/// whole solid sits in front of the image plane.
#[allow(clippy::too_many_arguments)]
pub fn raytrace_ortho(
    model: &BRepModel,
    solid_id: SolidId,
    center: Point3,
    right: Vector3,
    up: Vector3,
    dir: Vector3,
    half_w: f64,
    back: f64,
    width: usize,
    height: usize,
) -> RaytraceFrame {
    let r = right.normalize().unwrap_or(Vector3::X);
    let u = up.normalize().unwrap_or(Vector3::Y);
    let d = dir.normalize().unwrap_or(Vector3::Z);
    let aspect = height as f64 / width.max(1) as f64;
    let half_h = half_w * aspect;

    let n = width * height;
    let mut frame = RaytraceFrame {
        width,
        height,
        hit: vec![false; n],
        face_id: vec![0; n],
        depth: vec![f64::INFINITY; n],
        normal: vec![[0.0; 3]; n],
    };

    for py in 0..height {
        // image-plane v: top row (py=0) is +half_h, bottom is -half_h.
        let sv = 1.0 - (py as f64 + 0.5) / height as f64 * 2.0;
        for px in 0..width {
            let su = (px as f64 + 0.5) / width as f64 * 2.0 - 1.0;
            let origin = center + r * (su * half_w) + u * (sv * half_h) - d * back;
            if let Some(hit) = raycast_solid(model, solid_id, origin, d) {
                let i = py * width + px;
                frame.hit[i] = true;
                frame.face_id[i] = hit.face_id;
                frame.depth[i] = hit.distance;
                frame.normal[i] = [hit.normal.x, hit.normal.y, hit.normal.z];
            }
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    /// Look straight down −Z at a 20³ box (z ∈ [−10, 10]) centred at the origin.
    fn top_down(m: &BRepModel, s: SolidId, w: usize) -> RaytraceFrame {
        raytrace_ortho(
            m,
            s,
            Point3::ZERO,
            Vector3::X,                   // right
            Vector3::Y,                   // up
            Vector3::new(0.0, 0.0, -1.0), // dir (looking down)
            12.0,                         // half width (box half is 10)
            30.0,                         // ray origins at z = +30
            32,
            32,
        )
    }

    #[test]
    fn raytrace_box_top_is_exact_and_fully_covered() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let f = top_down(&m, b, 32);
        // The top face fills the frame (box 20 wide vs 24 frame → high coverage).
        assert!(
            f.coverage() > 0.6,
            "top face fills most of the frame: {}",
            f.coverage()
        );
        let c = f.idx(16, 16);
        assert!(f.hit[c], "centre ray hits");
        // Exact: ray from z=30 down hits the top at z=10 → depth 20; normal +Z.
        assert!(
            (f.depth[c] - 20.0).abs() < 1e-6,
            "centre depth {} != 20",
            f.depth[c]
        );
        assert!(f.normal[c][2] > 0.999, "top normal +Z: {:?}", f.normal[c]);
        // Recoverable: every visible face id is a real face of the solid.
        let real: std::collections::HashSet<u32> = {
            let solid = m.solids.get(b).unwrap();
            let shell = m.shells.get(solid.outer_shell).unwrap();
            shell.faces.iter().copied().collect()
        };
        for fid in f.visible_faces() {
            assert!(real.contains(&fid), "visible face {fid} is a real face");
        }
    }

    #[test]
    fn missing_top_face_reveals_see_through_depth_jump() {
        // THE soundness property at the image level: remove the top face and the
        // centre pixel must NOT keep reporting depth 20 — it sees through to the
        // bottom cap (depth 40) or misses. A tessellation render would still
        // show a plausible top; the analytic eye cannot.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let top = {
            let f = top_down(&m, b, 8);
            f.face_id[f.idx(4, 4)]
        };
        let shell_id = m.solids.get(b).unwrap().outer_shell;
        if let Some(shell) = m.shells.get_mut(shell_id) {
            shell.faces.retain(|&fid| fid != top);
        }
        let f = top_down(&m, b, 8);
        let c = f.idx(4, 4);
        if f.hit[c] {
            assert_ne!(f.face_id[c], top, "must not hit the removed face");
            assert!(
                f.depth[c] > 20.0 + 1e-3,
                "see-through: depth jumps past the removed top (got {})",
                f.depth[c]
            );
        }
    }

    #[test]
    fn raytrace_is_deterministic() {
        let mut m = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 10.0)
            .expect("sphere"));
        let a = raytrace_ortho(
            &m,
            s,
            Point3::ZERO,
            Vector3::X,
            Vector3::Y,
            Vector3::new(0.0, 0.0, -1.0),
            12.0,
            30.0,
            24,
            24,
        );
        let b = raytrace_ortho(
            &m,
            s,
            Point3::ZERO,
            Vector3::X,
            Vector3::Y,
            Vector3::new(0.0, 0.0, -1.0),
            12.0,
            30.0,
            24,
            24,
        );
        assert_eq!(a.depth, b.depth, "depth buffer stable");
        assert_eq!(a.face_id, b.face_id, "face-id buffer stable");
    }
}
