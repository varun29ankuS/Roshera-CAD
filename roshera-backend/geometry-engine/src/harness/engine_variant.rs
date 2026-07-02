//! Parametric rocket-engine VARIANT recipe (Move 3).
//!
//! One variant = TWO solids in one [`BRepModel`], built entirely from proven
//! kernel primitives (no REST, no server):
//!
//! 1. **Chamber + nozzle** — one smooth axisymmetric shell from
//!    [`revolve_smooth_nozzle`]: the inner flow contour (chamber cylinder →
//!    converging throat → diverging bell) is offset radially by `wall_t` and
//!    revolved into a single watertight wall.
//! 2. **Injector faceplate** — an analytic cylinder ([`TopologyBuilder::create_cylinder_3d`])
//!    drilled by `hole_count` bores on a ring at `ring_frac × plate_radius`, each
//!    a boolean **difference** with an analytic cylinder (the known-watertight
//!    single-bore path).
//!
//! The recipe is the substrate for a certified autonomous sweep: every variant
//! is either **REFUSED** (an op rejects the input — a typed [`VariantRefusal`])
//! or built and then **certified** ([`certify_variant`]) against the kernel's
//! ambient [`ValidityCertificate`]. A built-but-unsound variant is a genuine
//! geometry failure the certificate catches — not a scripted one. The objective
//! (design §4) is minimal wall material at fixed internal volume inside a fixed
//! envelope, all three read straight off the kernel: wall material via
//! [`BRepModel::calculate_solid_volume`], internal volume via a throwaway
//! [`revolve_smooth_solid`] void built from the SAME contour (self-consistent
//! integrator), envelope via [`BRepModel::solid_world_bbox`].

use crate::math::bbox::BBox;
use crate::math::vector3::{Point3, Vector3};
use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use crate::operations::revolve::{revolve_smooth_nozzle, revolve_smooth_solid, RevolveOptions};
use crate::primitives::provenance::ValidityCertificate;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// The seven-plus-layout parameters that define one engine variant. Radii and
/// lengths are in model units; the axis is +Z.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineParams {
    /// Throat radius `Rt` (the contour minimum).
    pub throat_r: f64,
    /// Expansion ratio `ε` — the bell exit radius is `Rt·√ε`.
    pub expansion_ratio: f64,
    /// Chamber radius `Rc`.
    pub chamber_r: f64,
    /// Chamber length / diameter ratio `L/D` (chamber length = `L/D · 2Rc`).
    pub chamber_l_over_d: f64,
    /// Wall thickness `t` — the radial offset of the outer wall.
    pub wall_t: f64,
    /// Number of injector holes on the ring.
    pub hole_count: usize,
    /// Injector hole radius `rh`.
    pub hole_r: f64,
    /// Ring-radius fraction `fr` — holes sit at `fr × plate_radius`.
    pub ring_frac: f64,
}

impl EngineParams {
    /// A filesystem-safe label for artifacts (no spaces, no dots).
    pub fn label(&self) -> String {
        format!(
            "rt{}_eps{}_rc{}_ld{}_t{}_n{}_rh{}_fr{}",
            fmt(self.throat_r),
            fmt(self.expansion_ratio),
            fmt(self.chamber_r),
            fmt(self.chamber_l_over_d),
            fmt(self.wall_t),
            self.hole_count,
            fmt(self.hole_r),
            fmt(self.ring_frac),
        )
    }

    /// Outer radius of the plate = outer chamber radius, so the faceplate caps
    /// the chamber head.
    fn plate_radius(&self) -> f64 {
        self.chamber_r + self.wall_t
    }

    /// Faceplate thickness — a fixed fraction of the chamber radius (deterministic,
    /// not a free parameter): thick enough to drill through, thin relative to the
    /// engine length.
    fn plate_thickness(&self) -> f64 {
        (self.chamber_r * 0.3).max(2.0 * self.wall_t)
    }

    /// Exit (bell) radius `Re = Rt·√ε`.
    fn exit_radius(&self) -> f64 {
        self.throat_r * self.expansion_ratio.max(0.0).sqrt()
    }

    /// The inner flow contour `(r, z)`, inlet→exit, every `r > 0`: chamber
    /// cylinder section → smooth converging throat → smooth diverging bell. Sampled
    /// densely enough for the cubic fit in [`revolve_smooth_nozzle`]; smoothstep
    /// tapers avoid sharp corners the fit would round anyway.
    fn inner_contour(&self) -> Vec<(f64, f64)> {
        let rc = self.chamber_r;
        let rt = self.throat_r;
        let re = self.exit_radius();
        let lc = self.chamber_l_over_d * 2.0 * rc; // chamber length
        let lconv = (rc - rt).abs().max(rt); // converging length
        let ldiv = (2.0 * (re - rt)).abs().max(rt); // diverging length

        // Smoothstep 3t²−2t³.
        let smooth = |t: f64| t * t * (3.0 - 2.0 * t);

        let mut pts: Vec<(f64, f64)> = Vec::new();
        // Chamber cylinder section.
        pts.push((rc, 0.0));
        pts.push((rc, lc));
        // Converging section (chamber radius → throat).
        let nconv = 4usize;
        for k in 1..=nconv {
            let t = k as f64 / nconv as f64;
            let z = lc + t * lconv;
            let r = rc + (rt - rc) * smooth(t);
            pts.push((r, z));
        }
        // Diverging bell (throat → exit).
        let ndiv = 5usize;
        for k in 1..=ndiv {
            let t = k as f64 / ndiv as f64;
            let z = lc + lconv + t * ldiv;
            let r = rt + (re - rt) * smooth(t);
            pts.push((r, z));
        }
        pts
    }
}

/// A built variant: the two solid ids plus the geometry needed to measure it.
#[derive(Debug, Clone)]
pub struct EngineVariant {
    /// Chamber + nozzle shell solid.
    pub chamber_nozzle: SolidId,
    /// Drilled injector faceplate solid.
    pub injector_plate: SolidId,
    /// The inner flow contour (retained so the internal-volume void is built from
    /// the identical contour — same integrator both sides).
    pub inner_rz: Vec<(f64, f64)>,
    /// Faceplate outer radius.
    pub plate_radius: f64,
    /// Faceplate thickness.
    pub plate_thickness: f64,
    /// Overall axial length (plate + chamber + nozzle).
    pub total_length: f64,
}

/// An op rejected the input BEFORE producing a solid — a clean, typed refusal.
/// Distinct from a built-but-unsound variant (which [`certify_variant`] catches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantRefusal {
    /// Parameter values are non-physical before any op ran.
    InvalidParams(String),
    /// `revolve_smooth_nozzle` rejected the contour / wall thickness.
    Revolve(String),
    /// The analytic plate primitive was rejected.
    PlatePrimitive(String),
    /// A hole-drilling boolean difference was rejected.
    HoleDrill(String),
}

impl std::fmt::Display for VariantRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VariantRefusal::InvalidParams(m) => write!(f, "invalid params: {m}"),
            VariantRefusal::Revolve(m) => write!(f, "revolve refused: {m}"),
            VariantRefusal::PlatePrimitive(m) => write!(f, "plate primitive refused: {m}"),
            VariantRefusal::HoleDrill(m) => write!(f, "hole drill refused: {m}"),
        }
    }
}

/// Build one variant into `model`. Returns the two solid ids + measures, or a
/// typed [`VariantRefusal`] if any op rejects the input.
pub fn build_variant(
    model: &mut BRepModel,
    p: &EngineParams,
) -> Result<EngineVariant, VariantRefusal> {
    // Cheap physical sanity gate (the ops enforce more, but a clear typed refusal
    // is friendlier than a numeric-error string).
    if !(p.throat_r > 0.0)
        || !(p.chamber_r > 0.0)
        || !(p.wall_t > 0.0)
        || !(p.expansion_ratio > 0.0)
        || !(p.chamber_l_over_d > 0.0)
        || !(p.hole_r > 0.0)
        || !(p.ring_frac > 0.0)
    {
        return Err(VariantRefusal::InvalidParams(format!(
            "non-positive dimension in {p:?}"
        )));
    }

    let inner_rz = p.inner_contour();

    // 1. Chamber + nozzle shell (axis +Z, full revolution).
    let opts = RevolveOptions::default();
    let chamber_nozzle = revolve_smooth_nozzle(model, &inner_rz, p.wall_t, opts.clone())
        .map_err(|e| VariantRefusal::Revolve(format!("{e:?}")))?;

    // 2. Injector faceplate = analytic cylinder capping the chamber head. The
    //    chamber inlet is at z = 0; the plate occupies z ∈ [−thickness, 0].
    let plate_radius = p.plate_radius();
    let plate_thickness = p.plate_thickness();
    let plate_geom = TopologyBuilder::new(model)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, -plate_thickness),
            Vector3::Z,
            plate_radius,
            plate_thickness,
        )
        .map_err(|e| VariantRefusal::PlatePrimitive(format!("{e:?}")))?;
    let mut plate = match plate_geom {
        GeometryId::Solid(id) => id,
        other => {
            return Err(VariantRefusal::PlatePrimitive(format!(
                "cylinder primitive returned non-solid id {other:?}"
            )))
        }
    };

    // 3. Drill the injector holes on a ring. Each drill is an analytic cylinder
    //    spanning the full plate thickness (with overhang so the bore is a clean
    //    through-hole), subtracted from the running plate solid.
    let ring_r = p.ring_frac * plate_radius;
    let overhang = plate_thickness.max(1.0);
    let drill_h = plate_thickness + 2.0 * overhang;
    let bopts = BooleanOptions::default();
    for i in 0..p.hole_count {
        let theta = std::f64::consts::TAU * (i as f64) / (p.hole_count.max(1) as f64);
        let cx = ring_r * theta.cos();
        let cy = ring_r * theta.sin();
        let drill_geom = TopologyBuilder::new(model)
            .create_cylinder_3d(
                Point3::new(cx, cy, -plate_thickness - overhang),
                Vector3::Z,
                p.hole_r,
                drill_h,
            )
            .map_err(|e| VariantRefusal::HoleDrill(format!("hole {i}: {e:?}")))?;
        let drill = match drill_geom {
            GeometryId::Solid(id) => id,
            other => {
                return Err(VariantRefusal::HoleDrill(format!(
                    "hole {i}: cylinder returned non-solid id {other:?}"
                )))
            }
        };
        plate = boolean_operation(model, plate, drill, BooleanOp::Difference, bopts.clone())
            .map_err(|e| VariantRefusal::HoleDrill(format!("hole {i}: {e:?}")))?;
    }

    let contour_len = inner_rz.last().map(|&(_, z)| z).unwrap_or(0.0).max(0.0);
    let total_length = contour_len + plate_thickness;

    Ok(EngineVariant {
        chamber_nozzle,
        injector_plate: plate,
        inner_rz,
        plate_radius,
        plate_thickness,
        total_length,
    })
}

/// A design envelope: the variant must fit within `max_diameter` radially and
/// `max_length` axially.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    pub max_diameter: f64,
    pub max_length: f64,
}

/// The certified verdict on a built variant: soundness (both solids), which cert
/// dimensions failed, the objective volumes, and envelope fit.
#[derive(Debug, Clone)]
pub struct VariantVerdict {
    /// Both solids self-certify sound.
    pub cert_sound: bool,
    /// Names of the failed sound-affecting cert dimensions (deduplicated across
    /// both solids). Empty iff `cert_sound`.
    pub failed_dimensions: Vec<&'static str>,
    /// Wall-material volume = chamber+nozzle shell + faceplate. `None` if either
    /// solid has no computable (physical) volume.
    pub wall_material_volume: Option<f64>,
    /// Internal (chamber+nozzle) cavity volume — the solid of revolution of the
    /// inner contour, same integrator as the wall side.
    pub internal_volume: Option<f64>,
    /// The combined solid bounds fit inside the envelope.
    pub in_envelope: bool,
}

/// Certify a built variant: full ambient certificate per solid, objective volumes,
/// and envelope fit. Never panics.
pub fn certify_variant(
    model: &mut BRepModel,
    v: &EngineVariant,
    envelope: &Envelope,
) -> VariantVerdict {
    let cert_cn = model.certify_solid(v.chamber_nozzle);
    let cert_plate = model.certify_solid(v.injector_plate);
    let cert_sound = cert_cn.is_sound() && cert_plate.is_sound();

    let mut failed_dimensions: Vec<&'static str> = Vec::new();
    for name in failed_cert_dimensions(&cert_cn) {
        if !failed_dimensions.contains(&name) {
            failed_dimensions.push(name);
        }
    }
    for name in failed_cert_dimensions(&cert_plate) {
        if !failed_dimensions.contains(&name) {
            failed_dimensions.push(name);
        }
    }

    // Wall-material volume: both solids must yield a physical volume.
    let wall_material_volume = match (
        model.calculate_solid_volume(v.chamber_nozzle),
        model.calculate_solid_volume(v.injector_plate),
    ) {
        (Some(a), Some(b)) => Some(a + b),
        _ => None,
    };

    // Internal volume: a throwaway solid-of-revolution void from the SAME contour,
    // in its own model so it never pollutes the variant.
    let internal_volume = {
        let mut void_model = BRepModel::new();
        match revolve_smooth_solid(&mut void_model, &v.inner_rz, RevolveOptions::default()) {
            Ok(void) => void_model.calculate_solid_volume(void),
            Err(_) => None,
        }
    };

    // Envelope: union the two world bounds and compare extents.
    let in_envelope = match (
        model.solid_world_bbox(v.chamber_nozzle),
        model.solid_world_bbox(v.injector_plate),
    ) {
        (Some(a), Some(b)) => {
            let bb = union_bbox(&a, &b);
            let size = bb.size();
            let radial = size.x.max(size.y);
            radial <= envelope.max_diameter && size.z <= envelope.max_length
        }
        _ => false,
    };

    VariantVerdict {
        cert_sound,
        failed_dimensions,
        wall_material_volume,
        internal_volume,
        in_envelope,
    }
}

/// The sound-affecting cert dimensions that failed, as stable names — mirrors
/// [`ValidityCertificate::is_sound`] exactly.
fn failed_cert_dimensions(cert: &ValidityCertificate) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    if !cert.brep_valid {
        out.push("brep_valid");
    }
    if !cert.watertight {
        out.push("watertight");
    }
    if !cert.manifold {
        out.push("manifold");
    }
    if !cert.oriented {
        out.push("oriented");
    }
    if !cert.self_intersection_free {
        out.push("self_intersection_free");
    }
    if !cert.construction_consistent.is_sound() {
        out.push("construction_consistent");
    }
    if !cert.eyes_consistent.is_sound() {
        out.push("eyes_consistent");
    }
    if !cert.tessellation.clean {
        out.push("tessellation");
    }
    if !cert.mesh_quality.clean {
        out.push("mesh_quality");
    }
    out
}

/// Axis-aligned union of two boxes.
fn union_bbox(a: &BBox, b: &BBox) -> BBox {
    BBox {
        min: a.min.min(&b.min),
        max: a.max.max(&b.max),
    }
}

/// Compact numeric label component: trims a trailing `.0`, replaces `.`/`-`.
fn fmt(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', "p").replace('-', "n")
}
