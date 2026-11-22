//! CNC turning (lathe) rule pack — "can this be turned, and what needs a
//! second (milling) op?"
//!
//! ## The spindle axis is found or refused, never assumed
//!
//! [`CncTurningParams::spindle_axis`] may be given. When it is not, the axis
//! is INFERRED: every convex (OD) cylinder of the part must lie on ONE axis
//! line, and that line is the spindle. No convex cylinder, or two different
//! OD axes, is a typed refusal on every axis-dependent rule — a STEP part
//! not centred on the origin is never judged against `+Z` through the
//! origin.
//!
//! | rule | claim |
//! |---|---|
//! | `turn.axisymmetry` | every face is a surface of revolution about the spindle: a coaxial cylinder/cone/torus, a sphere centred on the axis, or a plane perpendicular to it bounded only by coaxial circles. Any other face (a flat, a cross hole, a keyway, a hex) needs milling / live tooling. Measured: the COUNT of such faces, limit 0. Freeform faces refuse. |
//! | `turn.bore_depth_ratio` | every coaxial internal bore: length ÷ diameter ≤ `max_bore_depth_ratio` (boring-bar overhang). |
//! | `turn.min_wall` | every proven wall pair ≥ `min_wall`. |
//!
//! Not checked: groove/undercut insert widths, thread relief, part
//! length-to-diameter (tailstock support), chucking stock.

use crate::dfm::analyzers::features::{
    concave_features, dot3, rim_span, ConcaveFeatures, PARALLEL_COS,
};
use crate::dfm::packs::cnc::evaluate_min_wall;
use crate::dfm::packs::common::{
    analytic, analytic_on, solid_face_ids, unit_direction, RuleFold, Sense,
};
use crate::dfm::packs::{Rule, RulePack};
use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{
    CncTurningParams, DfmError, DfmReport, PackParams, RuleVerdict, SurfaceKind, UnverifiableReason,
};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::FaceGeometry;

pub const AXISYMMETRY_RULE_ID: &str = "turn.axisymmetry";
pub const BORE_DEPTH_RATIO_RULE_ID: &str = "turn.bore_depth_ratio";
pub const MIN_WALL_RULE_ID: &str = "turn.min_wall";

fn practice(note: &str) -> RuleProvenance {
    RuleProvenance::ShopPractice {
        note: note.to_string(),
    }
}
pub fn axisymmetry_provenance() -> RuleProvenance {
    practice(
        "a lathe produces only surfaces of revolution about its spindle (geometric fact); any \
         other face needs a milling or live-tool operation",
    )
}
pub fn bore_depth_ratio_provenance() -> RuleProvenance {
    practice(
        "steel-shank boring bars hold ~4x their diameter of overhang (default 4; carbide ~6x, \
         damped ~10x) — tooling-vendor practice, no governing standard",
    )
}
pub fn min_wall_provenance() -> RuleProvenance {
    practice("thin walls deflect and chatter under the tool; default 0.8 mm — shop practice")
}

pub fn rule_pack(params: PackParams) -> RulePack {
    let r = |id, provenance| Rule { id, provenance };
    RulePack {
        params,
        rules: vec![
            r(AXISYMMETRY_RULE_ID, axisymmetry_provenance()),
            r(BORE_DEPTH_RATIO_RULE_ID, bore_depth_ratio_provenance()),
            r(MIN_WALL_RULE_ID, min_wall_provenance()),
        ],
    }
}

/// The spindle axis line `(point, unit direction)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpindleAxis {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

impl SpindleAxis {
    fn distance_to(&self, p: [f64; 3]) -> f64 {
        let d = [
            p[0] - self.origin[0],
            p[1] - self.origin[1],
            p[2] - self.origin[2],
        ];
        let s = dot3(d, self.direction);
        let off = [
            d[0] - s * self.direction[0],
            d[1] - s * self.direction[1],
            d[2] - s * self.direction[2],
        ];
        dot3(off, off).sqrt()
    }
    fn tol(&self, size: f64) -> f64 {
        1e-6 * size.max(1.0)
    }
    /// The line `(o, k)` is this axis.
    pub fn is_line(&self, o: [f64; 3], k: [f64; 3], size: f64) -> bool {
        dot3(k, self.direction).abs() >= PARALLEL_COS && self.distance_to(o) <= self.tol(size)
    }
}

/// Infer the spindle axis from the part's convex (OD) cylinders.
pub fn infer_spindle_axis(features: &ConcaveFeatures) -> Result<SpindleAxis, String> {
    let mut axis: Option<(SpindleAxis, f64)> = None;
    for n in &features.graph.nodes {
        if n.cavity != Some(false) {
            continue;
        }
        let FaceGeometry::Cylinder {
            origin,
            axis: k,
            radius,
        } = &n.geometry
        else {
            continue;
        };
        match &axis {
            None => {
                axis = Some((
                    SpindleAxis {
                        origin: *origin,
                        direction: *k,
                    },
                    *radius,
                ))
            }
            Some((a, r)) => {
                if !a.is_line(*origin, *k, r.max(*radius)) {
                    return Err(format!(
                        "convex cylinders lie on more than one axis (face {} is off the first \
                         OD's axis): no unique spindle axis",
                        n.face
                    ));
                }
            }
        }
    }
    axis.map(|(a, _)| a).ok_or_else(|| {
        "the part has no convex (OD) cylinder to infer a spindle axis from; pass spindle_axis"
            .to_string()
    })
}

/// Faces that are NOT surfaces of revolution about `axis`, and faces
/// whose geometry cannot be decided.
pub fn off_axis_faces(
    model: &BRepModel,
    features: &ConcaveFeatures,
    axis: &SpindleAxis,
) -> (Vec<FaceId>, Vec<FaceId>) {
    let mut off = Vec::new();
    let mut unknown = Vec::new();
    for n in &features.graph.nodes {
        let ok = match &n.geometry {
            FaceGeometry::Plane { normal, .. } => {
                dot3(*normal, axis.direction).abs() >= PARALLEL_COS
                    && plane_bounded_by_coaxial_circles(model, n.face, axis)
            }
            FaceGeometry::Cylinder {
                origin,
                axis: k,
                radius,
            } => axis.is_line(*origin, *k, *radius),
            FaceGeometry::Cone { apex, axis: k, .. } => axis.is_line(*apex, *k, 1.0),
            FaceGeometry::Torus {
                center,
                axis: k,
                major_radius,
                ..
            } => axis.is_line(*center, *k, *major_radius),
            FaceGeometry::Sphere { center, radius } => {
                axis.distance_to(*center) <= axis.tol(*radius)
            }
            FaceGeometry::Other { .. } => {
                match crate::dfm::analyzers::features::fillet_cylinder(model, n.face) {
                    Some(fc) => axis.is_line(fc.origin, fc.axis, fc.radius),
                    None => {
                        unknown.push(n.face);
                        continue;
                    }
                }
            }
        };
        if !ok {
            off.push(n.face);
        }
    }
    (off, unknown)
}

fn plane_bounded_by_coaxial_circles(model: &BRepModel, face: FaceId, axis: &SpindleAxis) -> bool {
    let Some(f) = model.faces.get(face) else {
        return false;
    };
    let mut loops = vec![f.outer_loop];
    loops.extend_from_slice(&f.inner_loops);
    loops.iter().all(|&lid| {
        model.loops.get(lid).is_some_and(|lp| {
            lp.edges
                .iter()
                .all(|&e| rim_span(model, e, axis.origin, axis.direction).is_some())
        })
    })
}

fn refuse_all(
    rule: &'static str,
    prov: RuleProvenance,
    faces: &[FaceId],
    why: &str,
) -> RuleVerdict {
    let mut fold = RuleFold::new(rule, prov, Sense::AtMost, 0.0, "n/a: no spindle axis");
    fold.refuse(
        faces.to_vec(),
        UnverifiableReason::UnsupportedTopology {
            detail: why.to_string(),
        },
    );
    fold.finish()
}

pub fn evaluate(
    model: &BRepModel,
    solid_id: SolidId,
    params: &CncTurningParams,
) -> Result<DfmReport, DfmError> {
    let faces = solid_face_ids(model, solid_id)?;
    let features = concave_features(model, solid_id)?;
    let axis = match params.spindle_axis {
        Some((o, k)) => Ok(SpindleAxis {
            origin: o,
            direction: unit_direction(k, "spindle axis direction")?,
        }),
        None => infer_spindle_axis(&features),
    };

    let (axisymmetry, bore) = match &axis {
        Err(why) => (
            refuse_all(AXISYMMETRY_RULE_ID, axisymmetry_provenance(), &faces, why),
            refuse_all(
                BORE_DEPTH_RATIO_RULE_ID,
                bore_depth_ratio_provenance(),
                &faces,
                why,
            ),
        ),
        Ok(axis) => {
            let (off, unknown) = off_axis_faces(model, &features, axis);
            let mut sym = RuleFold::new(
                AXISYMMETRY_RULE_ID,
                axisymmetry_provenance(),
                Sense::AtMost,
                0.0,
                "turn.axisymmetry limit: no face off the spindle's revolution",
            );
            sym.observe(
                off.clone(),
                off.len() as f64,
                analytic(format!(
                    "count of faces that are not surfaces of revolution about the spindle axis \
                     {:?} through {:?}",
                    axis.direction, axis.origin
                )),
            );
            for f in unknown {
                sym.refuse(
                    vec![f],
                    UnverifiableReason::UnsupportedTopology {
                        detail: format!(
                            "face {f} is freeform: whether it is a surface of revolution about \
                             the spindle is not decided"
                        ),
                    },
                );
            }
            let mut bore = RuleFold::new(
                BORE_DEPTH_RATIO_RULE_ID,
                bore_depth_ratio_provenance(),
                Sense::AtMost,
                params.max_bore_depth_ratio,
                "turn.bore_depth_ratio limit: max_bore_depth_ratio (pack params)",
            );
            for c in &features.cavity_cylinders {
                if axis.is_line(c.origin, c.axis, c.radius) && c.radius > 0.0 {
                    bore.observe(
                        c.faces.clone(),
                        c.length() / (2.0 * c.radius),
                        analytic_on(
                            SurfaceKind::Cylinder,
                            format!(
                                "coaxial bore length {:.4} / diameter {:.4}",
                                c.length(),
                                2.0 * c.radius
                            ),
                        ),
                    );
                }
            }
            (sym.finish(), bore.finish())
        }
    };
    let min_wall = evaluate_min_wall(
        model,
        &faces,
        params.min_wall,
        MIN_WALL_RULE_ID,
        min_wall_provenance(),
    )?;
    Ok(DfmReport::new(
        PackParams::CncTurning(params.clone()),
        vec![axisymmetry, bore, min_wall],
    ))
}
