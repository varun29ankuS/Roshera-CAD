//! Exact analytic mass properties by per-face Gaussian quadrature over the
//! trimmed B-rep surfaces (divergence theorem). Replaces the mesh-tetrahedra
//! path and the one-point/box-approximation analytic path.
//!
//! For an outward-oriented boundary `∂V`, every volume/moment integral reduces
//! to a surface integral, with `n dA = sign·(r_u×r_v) du dv` and
//! `sign = face.orientation.sign()`. One quadrature pass accumulates volume,
//! area, first moments (→ centre of mass) and all six second moments (→ full
//! inertia tensor). See
//! `docs/superpowers/specs/2026-07-09-exact-analytic-mass-properties-design.md`.

use crate::math::{MathResult, Point3, Vector3};
use crate::primitives::face::Face;
use crate::primitives::solid::{
    compute_principal_inertia, MassPropertiesMethod, SolidMassProperties,
};
use crate::primitives::surface::Surface;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::surface::{get_face_parameter_bounds, is_point_inside_face};

const EPS: f64 = 1e-12;

// 8-point Gauss-Legendre on [-1, 1] (exact for polynomials up to degree 15).
const GL_N: usize = 8;
const GL_X: [f64; GL_N] = [
    -0.9602898564975363,
    -0.7966664774136267,
    -0.5255324099163290,
    -0.1834346424956498,
    0.1834346424956498,
    0.5255324099163290,
    0.7966664774136267,
    0.9602898564975363,
];
const GL_W: [f64; GL_N] = [
    0.1012285362903763,
    0.2223810344533745,
    0.3137066458778873,
    0.3626837833783620,
    0.3626837833783620,
    0.3137066458778873,
    0.2223810344533745,
    0.1012285362903763,
];

/// Raw divergence-theorem moment accumulators (about the world origin) for one
/// face or a whole solid. Composable via `+=`; inner shells contribute via
/// `scaled(-1.0)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FaceMoments {
    pub area: f64,
    pub volume: f64,
    /// ∫x dV, ∫y dV, ∫z dV
    pub m1: [f64; 3],
    /// ∫x² dV, ∫y² dV, ∫z² dV
    pub m2_diag: [f64; 3],
    /// ∫xy dV, ∫yz dV, ∫zx dV
    pub m2_prod: [f64; 3],
    /// Accumulated absolute error estimate from bounded trim cells.
    pub achieved_err: f64,
}

impl std::ops::AddAssign for FaceMoments {
    fn add_assign(&mut self, o: Self) {
        self.area += o.area;
        self.volume += o.volume;
        self.achieved_err += o.achieved_err;
        for i in 0..3 {
            self.m1[i] += o.m1[i];
            self.m2_diag[i] += o.m2_diag[i];
            self.m2_prod[i] += o.m2_prod[i];
        }
    }
}

impl FaceMoments {
    /// Scale the volume/moment terms (area is unsigned and left unchanged).
    fn scaled(mut self, s: f64) -> Self {
        self.volume *= s;
        for i in 0..3 {
            self.m1[i] *= s;
            self.m2_diag[i] *= s;
            self.m2_prod[i] *= s;
        }
        self
    }
}

/// Accumulate the divergence-theorem contribution of ONE quadrature node.
/// `cross = r_u × r_v` carries the area element; `sign` orients it outward.
fn accumulate_node(r: Point3, ru: Vector3, rv: Vector3, sign: f64, w: f64, out: &mut FaceMoments) {
    let cross = ru.cross(&rv);
    out.area += w * cross.magnitude(); // area uses the magnitude, no sign
    let (x, y, z) = (r.x, r.y, r.z);
    let (nx, ny, nz) = (sign * cross.x, sign * cross.y, sign * cross.z);
    // Volume (symmetric form): ⅓(x·nx + y·ny + z·nz)
    out.volume += w * (x * nx + y * ny + z * nz) / 3.0;
    // First moments: ∫x dV = ½ x² nx, etc.
    out.m1[0] += w * 0.5 * x * x * nx;
    out.m1[1] += w * 0.5 * y * y * ny;
    out.m1[2] += w * 0.5 * z * z * nz;
    // Second moments (diagonal): ∫x² dV = ⅓ x³ nx, etc.
    out.m2_diag[0] += w * x * x * x * nx / 3.0;
    out.m2_diag[1] += w * y * y * y * ny / 3.0;
    out.m2_diag[2] += w * z * z * z * nz / 3.0;
    // Products: ∫xy dV = ½ x² y nx ; ∫yz dV = ½ y² z ny ; ∫zx dV = ½ z² x nz
    out.m2_prod[0] += w * 0.5 * x * x * y * nx;
    out.m2_prod[1] += w * 0.5 * y * y * z * ny;
    out.m2_prod[2] += w * 0.5 * z * z * x * nz;
}

/// Fixed-order tensor-product Gauss quadrature over the UV cell
/// `[u0,u1]×[v0,v1]`. Nodes whose surface eval fails / is non-finite are
/// skipped (they contribute nothing).
fn quad_cell(surface: &dyn Surface, sign: f64, u0: f64, u1: f64, v0: f64, v1: f64) -> FaceMoments {
    let (hu, hv) = ((u1 - u0) * 0.5, (v1 - v0) * 0.5);
    let (mu, mv) = ((u1 + u0) * 0.5, (v1 + v0) * 0.5);
    let mut acc = FaceMoments::default();
    for i in 0..GL_N {
        let u = mu + hu * GL_X[i];
        for j in 0..GL_N {
            let v = mv + hv * GL_X[j];
            if let Ok(sp) = surface.evaluate_full(u, v) {
                let (r, ru, rv) = (sp.position, sp.du, sp.dv);
                if r.x.is_finite()
                    && r.y.is_finite()
                    && r.z.is_finite()
                    && ru.magnitude().is_finite()
                    && rv.magnitude().is_finite()
                {
                    let w = GL_W[i] * GL_W[j] * hu * hv;
                    accumulate_node(r, ru, rv, sign, w, &mut acc);
                }
            }
        }
    }
    acc
}

/// Uniform fixed pre-subdivision per axis (curvature / periodic resolution).
const SUBDIV_FIXED: usize = 8;
/// Hard cap on adaptive trim-boundary recursion. This is the entire
/// no-hangs guarantee — the subdivision can never run away.
const MAX_TRIM_DEPTH: usize = 10;

enum Cls {
    In,
    Out,
    Straddle,
}

/// Classify a UV cell against the face's trim, sampling 4 corners + centre
/// INSET from the exact cell edges so a full-parameter (untrimmed) face — whose
/// loop coincides with the parameter boundary — classifies cleanly as `In`
/// rather than ambiguously straddling.
fn classify(face: &Face, model: &BRepModel, u0: f64, u1: f64, v0: f64, v1: f64) -> Cls {
    let e = 1.0e-3;
    let (lu, ru2) = (u0 + e * (u1 - u0), u1 - e * (u1 - u0));
    let (lv, rv2) = (v0 + e * (v1 - v0), v1 - e * (v1 - v0));
    let pts = [
        (lu, lv),
        (ru2, lv),
        (lu, rv2),
        (ru2, rv2),
        ((u0 + u1) * 0.5, (v0 + v1) * 0.5),
    ];
    let mut n_in = 0usize;
    for (u, v) in pts {
        if is_point_inside_face(u, v, face, model) {
            n_in += 1;
        }
    }
    match n_in {
        5 => Cls::In,
        0 => Cls::Out,
        _ => Cls::Straddle,
    }
}

/// Gauss quadrature over a cell, but each node kept only if inside the trim.
/// Used for boundary cells that hit the depth cap.
fn quad_cell_masked(
    face: &Face,
    surface: &dyn Surface,
    model: &BRepModel,
    sign: f64,
    u0: f64,
    u1: f64,
    v0: f64,
    v1: f64,
) -> FaceMoments {
    let (hu, hv) = ((u1 - u0) * 0.5, (v1 - v0) * 0.5);
    let (mu, mv) = ((u1 + u0) * 0.5, (v1 + v0) * 0.5);
    let mut acc = FaceMoments::default();
    for i in 0..GL_N {
        let u = mu + hu * GL_X[i];
        for j in 0..GL_N {
            let v = mv + hv * GL_X[j];
            if !is_point_inside_face(u, v, face, model) {
                continue;
            }
            if let Ok(sp) = surface.evaluate_full(u, v) {
                let (r, ru, rv) = (sp.position, sp.du, sp.dv);
                if r.x.is_finite()
                    && r.y.is_finite()
                    && r.z.is_finite()
                    && ru.magnitude().is_finite()
                    && rv.magnitude().is_finite()
                {
                    let w = GL_W[i] * GL_W[j] * hu * hv;
                    accumulate_node(r, ru, rv, sign, w, &mut acc);
                }
            }
        }
    }
    acc
}

/// Recursively integrate a UV cell: interior → exact Gauss; exterior → skip;
/// boundary → subdivide until the depth cap, then masked quadrature.
#[allow(clippy::too_many_arguments)]
fn integrate_cell(
    face: &Face,
    surface: &dyn Surface,
    model: &BRepModel,
    sign: f64,
    u0: f64,
    u1: f64,
    v0: f64,
    v1: f64,
    depth: usize,
    out: &mut FaceMoments,
) {
    match classify(face, model, u0, u1, v0, v1) {
        Cls::Out => {}
        Cls::In => *out += quad_cell(surface, sign, u0, u1, v0, v1),
        Cls::Straddle => {
            if depth >= MAX_TRIM_DEPTH {
                let masked = quad_cell_masked(face, surface, model, sign, u0, u1, v0, v1);
                let full = quad_cell(surface, sign, u0, u1, v0, v1);
                out.achieved_err += (full.volume - masked.volume).abs();
                *out += masked;
            } else {
                let (mu, mv) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
                integrate_cell(face, surface, model, sign, u0, mu, v0, mv, depth + 1, out);
                integrate_cell(face, surface, model, sign, mu, u1, v0, mv, depth + 1, out);
                integrate_cell(face, surface, model, sign, u0, mu, mv, v1, depth + 1, out);
                integrate_cell(face, surface, model, sign, mu, u1, mv, v1, depth + 1, out);
            }
        }
    }
}

/// Integrate one oriented, possibly-trimmed face: fixed pre-subdivision for
/// curvature resolution, each coarse cell routed through adaptive trim
/// classification.
pub fn integrate_face(face: &Face, model: &BRepModel, _tol: f64) -> MathResult<FaceMoments> {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return Ok(FaceMoments::default()),
    };
    let sign = face.orientation.sign();
    let (u0, u1, v0, v1) = get_face_parameter_bounds(face, model);
    if !(u0.is_finite() && u1.is_finite() && v0.is_finite() && v1.is_finite())
        || u1 <= u0
        || v1 <= v0
    {
        return Ok(FaceMoments::default());
    }
    let du = (u1 - u0) / SUBDIV_FIXED as f64;
    let dv = (v1 - v0) / SUBDIV_FIXED as f64;
    let mut acc = FaceMoments::default();
    for iu in 0..SUBDIV_FIXED {
        let cu0 = u0 + du * iu as f64;
        let cu1 = cu0 + du;
        for iv in 0..SUBDIV_FIXED {
            let cv0 = v0 + dv * iv as f64;
            let cv1 = cv0 + dv;
            integrate_cell(face, surface, model, sign, cu0, cu1, cv0, cv1, 0, &mut acc);
        }
    }
    Ok(acc)
}

/// Integrate a whole solid (outer shell +, inner shells −) then assemble.
pub fn integrate_solid(
    solid_id: u32,
    model: &BRepModel,
    density: f64,
    tol: f64,
) -> Option<SolidMassProperties> {
    let solid = model.solids.get(solid_id)?;
    let mut acc = FaceMoments::default();
    if let Some(shell) = model.shells.get(solid.outer_shell) {
        for &fid in &shell.faces {
            if let Some(face) = model.faces.get(fid) {
                if let Ok(fm) = integrate_face(face, model, tol) {
                    acc += fm;
                }
            }
        }
    }
    for &inner in &solid.inner_shells {
        if let Some(sh) = model.shells.get(inner) {
            for &fid in &sh.faces {
                if let Some(face) = model.faces.get(fid) {
                    if let Ok(fm) = integrate_face(face, model, tol) {
                        acc += fm.scaled(-1.0);
                    }
                }
            }
        }
    }
    Some(assemble(acc, density))
}

/// Assemble raw origin moments into a full `SolidMassProperties`: centre of
/// mass, inertia tensor (parallel-axis-shifted to the CoM), principal
/// moments/axes (Jacobi) and radius of gyration.
pub fn assemble(m: FaceMoments, density: f64) -> SolidMassProperties {
    let v = m.volume; // signed; + for an outward-oriented outer shell
    let vol = v.abs();
    // Normalise so the moments match a positive volume regardless of the
    // aggregate winding sign.
    let s = if v < 0.0 { -1.0 } else { 1.0 };
    let mass = vol * density;

    let com = if vol > EPS {
        Point3::new(s * m.m1[0] / vol, s * m.m1[1] / vol, s * m.m1[2] / vol)
    } else {
        Point3::ZERO
    };

    // Density-1 origin second moments, sign-normalised.
    let (ixx0, iyy0, izz0) = (s * m.m2_diag[0], s * m.m2_diag[1], s * m.m2_diag[2]);
    let (ixy0, iyz0, izx0) = (s * m.m2_prod[0], s * m.m2_prod[1], s * m.m2_prod[2]);

    // Inertia tensor about the origin (× density). Products of inertia negated.
    let d = density;
    let mut it = [
        [d * (iyy0 + izz0), -d * ixy0, -d * izx0],
        [-d * ixy0, d * (ixx0 + izz0), -d * iyz0],
        [-d * izx0, -d * iyz0, d * (ixx0 + iyy0)],
    ];

    // Parallel-axis shift origin → CoM: I_cm = I_o − m(|c|²·𝟙 − c cᵀ).
    let c = [com.x, com.y, com.z];
    let c2 = c[0] * c[0] + c[1] * c[1] + c[2] * c[2];
    for i in 0..3 {
        for j in 0..3 {
            let delta = if i == j { c2 } else { 0.0 } - c[i] * c[j];
            it[i][j] -= mass * delta;
        }
    }

    let (principal_moments, principal_axes) = compute_principal_inertia(&it);
    let radius_of_gyration = if mass > EPS {
        Vector3::new(
            (principal_moments.x / mass).max(0.0).sqrt(),
            (principal_moments.y / mass).max(0.0).sqrt(),
            (principal_moments.z / mass).max(0.0).sqrt(),
        )
    } else {
        Vector3::ZERO
    };

    SolidMassProperties {
        volume: vol,
        surface_area: m.area,
        mass,
        center_of_mass: com,
        inertia_tensor: it,
        principal_moments,
        principal_axes,
        radius_of_gyration,
        method: MassPropertiesMethod::Analytical,
    }
}
