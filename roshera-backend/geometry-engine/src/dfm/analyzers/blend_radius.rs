//! `blend_radius` — internal (CONCAVE) corner radii: toroidal blend minor
//! radius, cylindrical fillet radius on a concave edge, and explicit
//! sharp-edge (radius 0) detection (spec §3.1 / S5).
//!
//! ## Concavity is DERIVED, reused from the kernel's own convention — never
//! re-invented here
//!
//! The kernel already classifies edge convexity:
//! [`crate::operations::edge_classification::classify_edge`] computes a
//! signed dihedral from evaluated face normals + a geometry-derived tangent
//! (`DihedralClass::{Convex, Concave, G1Smooth}`, `edge_classification.rs`
//! module docs). This analyzer's SHARP-CORNER arm (radius 0) calls it
//! directly — a concave edge with no blend surface IS the reportable
//! result, full stop.
//!
//! The BLEND-FACE arm (a genuine toroidal/cylindrical fillet) needs a
//! per-FACE concave/convex verdict, which `classify_edge` does not produce
//! (it classifies edges, not faces). For `Cylinder`, the kernel already has
//! an in-tree convention for this: [`crate::readable::bore_face_ids`]
//! (`readable/dimensions.rs:400`) treats a cylindrical face as concave
//! (material outside, void toward the axis) iff
//! `face.orientation == FaceOrientation::Backward` — the same convention
//! [`crate::dfm::analyzers::thickness`]'s `Classified::Cylinder` doc states
//! independently ("`sign == -1` (`Backward`) means the outward normal
//! points TOWARD the axis — material is OUTSIDE"). That predicate is
//! inlined inside `bore_face_ids` (no standalone function to call), so this
//! module reuses the CONVENTION (cited above), not a callable — the
//! no-copies discipline is satisfied by citation + identical logic, not by
//! re-deriving the physics from scratch.
//!
//! **Torus extends the same convention exactly**, verified by hand from
//! [`crate::primitives::surface::Torus::evaluate_full`], not assumed:
//! at `u=0, v=0` the surface point is `center + x_dir*(R+r)` (the outer
//! equatorial point) and `normal = du × dv` reduces to `x_dir` — i.e. the
//! natural (pre-orientation) normal equals `tube_radial = cos(v)·radial_dir(u)
//! + sin(v)·axis`, the direction from the LOCAL tube-circle center
//! (`major_center(u)`) to the surface point. This is the exact torus
//! analogue of a cylinder's `radial_dir` (which also points away from the
//! axis, the cylinder's own center of curvature). A second, independent
//! check at `v=π/2` (any `u`) gives `normal = +axis` directly from
//! `du × dv` (`du = R·tangential(u)`, `dv = -r·radial_dir(u)`,
//! `tangential(u) × radial_dir(u) = -axis` ⇒ `normal = R·r·axis`,
//! normalized to `+axis`, which is `tube_radial` at `v=π/2` by definition).
//! So `face.orientation == FaceOrientation::Backward` on a `Torus` face
//! means the outward normal points TOWARD the tube's local center of
//! curvature — exactly the "material outside, curvature center in the
//! void" shape a concave blend has, by the SAME center-of-curvature
//! argument that makes a bore wall concave: the outward normal (material→
//! void) points toward the axis/tube-center iff that axis/tube-center is
//! itself in the void, which is what "concave" means.
//!
//! ## Distinguishing a genuine BLEND from a bore/pocket wall
//!
//! `Backward` alone is not enough: a bore's cylindrical wall is ALSO
//! `Backward` (concave) but is not a corner blend — it is a full hole, not
//! a tangent transition between two other faces. A genuine fillet is
//! TANGENT (G1-continuous) to the face(s) it blends by construction; a bore
//! wall meets its end-cap faces at a sharp (non-tangent) angle. This
//! analyzer uses [`classify_edge`]'s dihedral on the candidate face's own
//! boundary edges as that discriminator: at least one boundary edge must
//! classify [`DihedralClass::G1Smooth`] (dihedral within
//! `Tolerance::default().angle()` of the two face normals being parallel —
//! ≈0.1°, `math/tolerance.rs`'s `NORMAL_TOLERANCE`) for a `Backward`
//! Cylinder/Torus face to be reported as a blend at all. Without this gate,
//! EVERY bore wall (S4's own headline candidate) would also read as an
//! "internal corner radius" — a second silent-fabrication defect this
//! analyzer must not reintroduce.
//!
//! **This tangency gate is NOT the concavity discriminator** — it only
//! answers "is this face acting as a tangent blend transition at all". A
//! `Forward` (convex) face can be just as tangent as a `Backward` one (an
//! EXTERNAL fillet is tangent too); concavity is decided solely by
//! `face.orientation` as derived above. Conflating the two would be exactly
//! the "assumed, not derived" failure mode the executor brief warns about.
//!
//! **Known gap (honest, not coverage):** the tangency gate can silently
//! UNDER-report. A concave fillet whose true tangency falls outside the
//! ≈0.1° band (a slightly-off-tangent blend from upstream numerical noise),
//! or whose neighbour face does not share the SAME edge id (a topology
//! defect), is excluded rather than reported — it is neither a `blends`
//! entry nor an `unverifiable` one, matching S4's own "silently excluded,
//! not fabricated" precedent for non-candidate faces (`bore.rs` module
//! docs). Every fixture in this module's own tests is a hand-built bare
//! store (the established convention — booleans/`fillet_edges` are a
//! `KNOWN_REDS` hazard area these tests avoid, same as every other `dfm`
//! analyzer), so none of them exercise a REAL `operations::fillet`-produced
//! blend; this gap is stated, not measured, against that real path.
//!
//! ## Freeform blends refuse, named (spec §3.1)
//!
//! A CURVED face whose surface kind is none of `{Cylinder, Torus}` but
//! which STILL passes the same tangency gate (evidence it is acting as a
//! blend transition, not an unrelated face elsewhere on the part) is
//! `Unverifiable{UnsupportedSurface}`, naming the kind — this covers
//! `SurfaceOfRevolution`/`BSpline`/`Nurbs`/`Offset`/`Ruled` (freeform G2
//! blends) and `Cone`/`Sphere` (analytic, but not in spec §3.1's
//! exact-support list for THIS analyzer). A non-tangent face of any of
//! these kinds is simply not a candidate (silently excluded, same
//! reasoning as the Cylinder/Torus case above).
//!
//! **`Plane` is excluded from this arm entirely, unconditionally** — not
//! merely gated on tangency. Tangency is a property of the shared EDGE,
//! symmetric between both faces either side of it: the ORDINARY flat
//! wall/deck a genuine blend is tangent to (this module's own `Wall`/`Deck`
//! test fixtures) satisfies the SAME `face_has_tangent_boundary_edge` test
//! the blend surface itself does. A plane can never be the curved blend
//! surface the spec's freeform-refuses arm targets, so treating a tangent
//! Plane as a refusal candidate would flag every ordinary flat neighbour of
//! every real blend — the mirror-image fabrication bug (over-refusal
//! instead of over-report) to the headline this module exists to prevent.
//!
//! ## Sharp corners are a real result, not a refusal
//!
//! A concave edge with no blend surface (`classify_edge` reports
//! `DihedralClass::Concave` directly) is reported with radius `0.0` and
//! `Derivation::Analytic` — this is the exact answer, not a placeholder.

use crate::dfm::analyzers::bore::solid_faces;
use crate::dfm::analyzers::orientation::to_surface_kind;
use crate::dfm::report::{
    Derivation, DfmError, DfmValue, FaceRef, SurfaceKind, UnverifiableReason,
};
use crate::operations::edge_classification::{classify_edge, DihedralClass};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::{FaceId, FaceOrientation};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cylinder, SurfaceType, Torus};
use crate::primitives::topology_builder::BRepModel;
use std::collections::BTreeSet;

/// One proven internal blend (spec §3.1): a `Cylinder`/`Torus` face that is
/// BOTH concave (module docs' `face.orientation` convention) AND tangent to
/// at least one neighbour (the anti-bore gate) — its radius read directly
/// off the carrier surface (`cyl.radius` / `torus.minor_radius`), no
/// derived math needed.
#[derive(Debug, Clone, PartialEq)]
pub struct BlendRecord {
    pub face: FaceRef,
    pub radius: DfmValue,
}

/// A real, reportable sharp internal corner: a concave edge
/// (`DihedralClass::Concave`) with no blend surface. `radius` is always
/// exactly `0.0` — this is the correct answer, not a missing one.
#[derive(Debug, Clone, PartialEq)]
pub struct SharpCorner {
    pub edge: EdgeId,
    pub radius: DfmValue,
}

/// A face that is acting as a blend transition (module docs' tangency
/// gate) on a surface kind this analyzer has no closed-form radius for.
#[derive(Debug, Clone, PartialEq)]
pub struct UnverifiableBlend {
    pub face: FaceRef,
    pub reason: UnverifiableReason,
}

/// The full result of one `blend_radius` call (spec §4: a refusal is a
/// VALUE, never an error — [`DfmError`] is reserved for malformed input: a
/// dangling `solid_id`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlendRadiusOutcome {
    pub blends: Vec<BlendRecord>,
    pub sharp_corners: Vec<SharpCorner>,
    pub unverifiable: Vec<UnverifiableBlend>,
}

/// Whether ANY boundary edge (outer loop + every inner loop) of `face_id`
/// classifies [`DihedralClass::G1Smooth`] — module docs' anti-bore
/// tangency gate. A [`classify_edge`] failure on one edge contributes no
/// evidence either way (the walk simply continues to the next edge) rather
/// than aborting the whole face — matching the rest of `dfm`'s
/// "best-effort, never fabricate" posture for a discriminator that can
/// only ever make the analyzer MORE conservative (fewer candidates), never
/// less.
fn face_has_tangent_boundary_edge(model: &BRepModel, face_id: FaceId) -> bool {
    let Some(face) = model.faces.get(face_id) else {
        return false;
    };
    let mut edge_ids: Vec<EdgeId> = Vec::new();
    if let Some(lp) = model.loops.get(face.outer_loop) {
        edge_ids.extend(lp.edges.iter().copied());
    }
    for &inner in &face.inner_loops {
        if let Some(lp) = model.loops.get(inner) {
            edge_ids.extend(lp.edges.iter().copied());
        }
    }
    edge_ids.iter().any(|&edge_id| {
        matches!(
            classify_edge(model, edge_id)
                .ok()
                .and_then(|c| c.dihedral_class()),
            Some(DihedralClass::G1Smooth)
        )
    })
}

/// `blend_radius` (spec §3.1, S5): every proven internal blend and sharp
/// concave corner of `solid_id`, or an honest per-region refusal (module
/// docs). Returns `Err` only for a dangling `solid_id` (spec §4).
pub fn blend_radius(model: &BRepModel, solid_id: SolidId) -> Result<BlendRadiusOutcome, DfmError> {
    let faces =
        solid_faces(model, solid_id).ok_or(DfmError::DanglingSolidRef { solid: solid_id })?;

    let mut blends = Vec::new();
    let mut unverifiable = Vec::new();

    for &face_id in &faces {
        let Some(face) = model.faces.get(face_id) else {
            continue; // dangling face on a resolved solid: defensive skip, matches bore.rs's solid_faces leniency
        };
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue;
        };

        match surface.surface_type() {
            SurfaceType::Cylinder => {
                let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() else {
                    continue; // unreachable: surface_type() == Cylinder guarantees the downcast
                };
                if face.orientation != FaceOrientation::Backward {
                    continue; // convex/external -- not a candidate at all (module docs' headline)
                }
                if !face_has_tangent_boundary_edge(model, face_id) {
                    continue; // concave but not tangent to a neighbour -- e.g. a bore wall, not a blend
                }
                blends.push(BlendRecord {
                    face: face_id,
                    radius: DfmValue::new(
                        cyl.radius,
                        Derivation::Analytic {
                            surface_type: SurfaceKind::Cylinder,
                            method: "cylindrical fillet radius on a concave \
                                     (Backward-oriented, tangent-to-neighbour) edge"
                                .to_string(),
                        },
                    ),
                });
            }
            SurfaceType::Torus => {
                let Some(torus) = surface.as_any().downcast_ref::<Torus>() else {
                    continue; // unreachable: surface_type() == Torus guarantees the downcast
                };
                if face.orientation != FaceOrientation::Backward {
                    continue;
                }
                if !face_has_tangent_boundary_edge(model, face_id) {
                    continue;
                }
                blends.push(BlendRecord {
                    face: face_id,
                    radius: DfmValue::new(
                        torus.minor_radius,
                        Derivation::Analytic {
                            surface_type: SurfaceKind::Torus,
                            method: "toroidal blend minor radius on a concave \
                                     (Backward-oriented, tangent-to-neighbour) edge"
                                .to_string(),
                        },
                    ),
                });
            }
            SurfaceType::Plane => {
                // A FLAT neighbour of a real blend is tangent to it too
                // (tangency is a property of the shared EDGE, symmetric
                // between both faces) -- but a plane can never itself be
                // the curved blend surface the spec's freeform-refuses arm
                // targets. Without this exclusion, every ordinary flat
                // wall/deck a genuine Cylinder/Torus/freeform blend is
                // tangent to would ALSO read as an unsupported-surface
                // refusal -- a second silent-fabrication bug (over-
                // refusal, not over-report) this analyzer must not have.
                continue;
            }
            other => {
                // Any other CURVED surface kind acting as a tangent blend
                // transition (module docs: the freeform-refuses arm).
                // Silently excluded (no entry at all) when there is no
                // tangency evidence -- an ordinary, unrelated face
                // elsewhere on the part must not read as a refused blend.
                if face_has_tangent_boundary_edge(model, face_id) {
                    unverifiable.push(UnverifiableBlend {
                        face: face_id,
                        reason: UnverifiableReason::UnsupportedSurface {
                            surface_type: to_surface_kind(other),
                            analyzer: "blend_radius".to_string(),
                        },
                    });
                }
            }
        }
    }

    // Sharp concave corners: every edge belonging to the solid's own
    // faces, deduplicated, classified via the kernel's own dihedral
    // classifier -- a real, reportable radius-0 result, not a refusal.
    let mut edge_ids: BTreeSet<EdgeId> = BTreeSet::new();
    for &face_id in &faces {
        if let Some(face) = model.faces.get(face_id) {
            if let Some(lp) = model.loops.get(face.outer_loop) {
                edge_ids.extend(lp.edges.iter().copied());
            }
            for &inner in &face.inner_loops {
                if let Some(lp) = model.loops.get(inner) {
                    edge_ids.extend(lp.edges.iter().copied());
                }
            }
        }
    }

    let mut sharp_corners = Vec::new();
    for edge_id in edge_ids {
        if let Ok(classification) = classify_edge(model, edge_id) {
            if classification.dihedral_class() == Some(DihedralClass::Concave) {
                sharp_corners.push(SharpCorner {
                    edge: edge_id,
                    radius: DfmValue::new(
                        0.0,
                        Derivation::Analytic {
                            // Not tied to one specific surface kind (the
                            // edge is shared by two faces of possibly
                            // different kinds) -- follows `packs/fdm.rs`'s
                            // own `constant_derivation` precedent for a
                            // value that is not measured off a single
                            // carrier surface: the real meaning lives in
                            // `method`, not `surface_type`.
                            surface_type: SurfaceKind::Plane,
                            method: "sharp concave edge (kernel dihedral classification), \
                                     no blend surface present -- radius 0 by definition"
                                .to_string(),
                        },
                    ),
                });
            }
        }
    }

    blends.sort_by_key(|b| b.face);
    unverifiable.sort_by_key(|u| u.face);
    sharp_corners.sort_by_key(|s| s.edge);

    Ok(BlendRadiusOutcome {
        blends,
        sharp_corners,
        unverifiable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dfm::packs::fixtures::model_with_solid;
    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::{Arc, CurveStore, Line, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation, EdgeStore};
    use crate::primitives::face::{Face, FaceStore};
    use crate::primitives::r#loop::{Loop, LoopStore, LoopType};
    use crate::primitives::surface::{Plane, SurfaceStore};

    /// Bare-store bundle, the same convention every other `dfm` analyzer's
    /// test module uses.
    struct Stores {
        surfaces: SurfaceStore,
        faces: FaceStore,
        loops: LoopStore,
        edges: EdgeStore,
        curves: CurveStore,
    }

    impl Stores {
        fn new() -> Self {
            Self {
                surfaces: SurfaceStore::new(),
                faces: FaceStore::new(),
                loops: LoopStore::new(),
                edges: EdgeStore::new(),
                curves: CurveStore::new(),
            }
        }

        fn add_line_edge(&mut self, start: Point3, end: Point3) -> EdgeId {
            let curve_id = self.curves.add(Box::new(Line::new(start, end)));
            self.edges.add(Edge::new(
                0,
                0,
                1,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            ))
        }

        fn add_arc_edge(&mut self, center: Point3, axis: Vector3, radius: f64) -> EdgeId {
            let arc = Arc::circle(center, axis, radius).unwrap_or_else(|e| panic!("arc: {e}"));
            let curve_id = self.curves.add(Box::new(arc));
            self.edges.add(Edge::new(
                0,
                0,
                1,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            ))
        }
    }

    /// The concave-corner fillet fixture (module docs' full derivation):
    /// two walls meeting at a reentrant edge, rounded by a QUARTER cylinder
    /// of radius `r` whose axis sits at `(r, r, ·)` -- INSIDE the original
    /// void quadrant `x>0, y>0`. Hand-verified tangency: at the shared
    /// generatrix `x=0, y=r` the fillet's natural radial normal is `-X`;
    /// `Backward` flips it to `+X`, exactly matching the wall's own
    /// (constant) `+X` outward normal -- an EXACT (not approximate)
    /// tangency, so the dihedral is exactly 0 regardless of tangent-sign
    /// ambiguity (the two face normals are parallel, so `n1 x n2 = 0`
    /// identically).
    fn concave_cylinder_fillet_fixture(
        radius: f64,
        y_max: f64,
        h: f64,
    ) -> (BRepModel, SolidId, FaceId) {
        let mut s = Stores::new();

        // Wall: plane x=0, outward normal +X, spanning y in [r, y_max],
        // z in [0, h] -- the portion of the wall ABOVE the fillet's own
        // tangent point (0, r).
        let wall_plane = Plane::from_point_normal(Point3::new(0.0, radius, 0.0), Vector3::X)
            .unwrap_or_else(|e| panic!("plane: {e}"));
        let wall_surface = s.surfaces.add(Box::new(wall_plane));
        let wall_corners = [
            Point3::new(0.0, radius, 0.0),
            Point3::new(0.0, y_max, 0.0),
            Point3::new(0.0, y_max, h),
            Point3::new(0.0, radius, h),
        ];
        let mut wall_loop = Loop::new(0, LoopType::Outer);
        for i in 0..4 {
            let (start, end) = (wall_corners[i], wall_corners[(i + 1) % 4]);
            let edge_id = if i == 0 {
                // The shared tangent edge: SAME edge id the fillet face
                // below also references, so `find_adjacent_faces` finds
                // both faces from this one edge -- a real B-Rep shared
                // edge, not two coincident-but-distinct ones.
                s.add_line_edge(start, end)
            } else {
                s.add_line_edge(start, end)
            };
            wall_loop.add_edge(edge_id, true);
        }
        // Re-derive the shared edge id explicitly (it is `wall_loop`'s
        // first edge) for reuse on the fillet face below.
        let shared_edge = wall_loop.edges[0];
        let wall_outer_loop = s.loops.add(wall_loop);
        let wall_face = s.faces.add(Face::new(
            0,
            wall_surface,
            wall_outer_loop,
            FaceOrientation::Forward,
        ));

        // Fillet: cylinder axis (r, r, ·), radius r, Backward -- the
        // concave quarter arc from theta=180 deg (point (0,r)) to
        // theta=270 deg (point (r,0)).
        let axis_origin = Point3::new(radius, radius, 0.0);
        let cylinder = Cylinder::new(axis_origin, Vector3::Z, radius)
            .unwrap_or_else(|e| panic!("cylinder: {e}"));
        let fillet_surface = s.surfaces.add(Box::new(cylinder));

        let bottom_rim = Arc::new(
            axis_origin,
            Vector3::Z,
            radius,
            std::f64::consts::PI,
            std::f64::consts::FRAC_PI_2,
        )
        .unwrap_or_else(|e| panic!("arc: {e}"));
        let top_rim = Arc::new(
            axis_origin + Vector3::Z * h,
            Vector3::Z,
            radius,
            std::f64::consts::PI,
            std::f64::consts::FRAC_PI_2,
        )
        .unwrap_or_else(|e| panic!("arc: {e}"));
        // The OTHER generatrix (theta=270 deg, point (r,0)): not shared
        // with anything in this minimal fixture -- an orphaned boundary
        // edge, fine for this test (only the theta=180 deg edge needs to
        // be tangent).
        let other_generatrix_start = Point3::new(radius, 0.0, 0.0);
        let other_generatrix_end = Point3::new(radius, 0.0, h);

        let bottom_curve = s.curves.add(Box::new(bottom_rim));
        let bottom_edge = s.edges.add(Edge::new(
            0,
            0,
            1,
            bottom_curve,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let other_gen_edge = s.add_line_edge(other_generatrix_end, other_generatrix_start);
        let top_curve = s.curves.add(Box::new(top_rim));
        let top_edge = s.edges.add(Edge::new(
            0,
            0,
            1,
            top_curve,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));

        let mut fillet_loop = Loop::new(1, LoopType::Outer);
        fillet_loop.add_edge(bottom_edge, true);
        fillet_loop.add_edge(other_gen_edge, true);
        fillet_loop.add_edge(top_edge, true);
        fillet_loop.add_edge(shared_edge, true); // the SAME edge id as the wall's
        let fillet_outer_loop = s.loops.add(fillet_loop);
        let fillet_face = s.faces.add(Face::new(
            1,
            fillet_surface,
            fillet_outer_loop,
            FaceOrientation::Backward,
        ));

        let face_ids = [wall_face, fillet_face];
        let (model, solid_id) =
            model_with_solid(s.surfaces, s.faces, s.loops, s.edges, s.curves, &face_ids);
        (model, solid_id, fillet_face)
    }

    /// The convex-corner companion (module docs' anti-fabrication
    /// headline): SAME construction, but the fillet center sits INSIDE the
    /// original material (`-r,-r`), and `Forward` is the tangency-matching
    /// orientation -- a genuinely tangent, genuinely convex fillet that
    /// must NEVER be reported by `blend_radius`.
    fn convex_cylinder_fillet_fixture(
        radius: f64,
        y_min: f64,
        h: f64,
    ) -> (BRepModel, SolidId, FaceId) {
        let mut s = Stores::new();

        let wall_plane = Plane::from_point_normal(Point3::new(0.0, -radius, 0.0), Vector3::X)
            .unwrap_or_else(|e| panic!("plane: {e}"));
        let wall_surface = s.surfaces.add(Box::new(wall_plane));
        let wall_corners = [
            Point3::new(0.0, -radius, 0.0),
            Point3::new(0.0, y_min, 0.0),
            Point3::new(0.0, y_min, h),
            Point3::new(0.0, -radius, h),
        ];
        let mut wall_loop = Loop::new(0, LoopType::Outer);
        for i in 0..4 {
            let (start, end) = (wall_corners[i], wall_corners[(i + 1) % 4]);
            let edge_id = s.add_line_edge(start, end);
            wall_loop.add_edge(edge_id, true);
        }
        let shared_edge = wall_loop.edges[0];
        let wall_outer_loop = s.loops.add(wall_loop);
        let wall_face = s.faces.add(Face::new(
            0,
            wall_surface,
            wall_outer_loop,
            FaceOrientation::Forward,
        ));

        let axis_origin = Point3::new(-radius, -radius, 0.0);
        let cylinder = Cylinder::new(axis_origin, Vector3::Z, radius)
            .unwrap_or_else(|e| panic!("cylinder: {e}"));
        let fillet_surface = s.surfaces.add(Box::new(cylinder));

        let bottom_rim = Arc::new(
            axis_origin,
            Vector3::Z,
            radius,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .unwrap_or_else(|e| panic!("arc: {e}"));
        let top_rim = Arc::new(
            axis_origin + Vector3::Z * h,
            Vector3::Z,
            radius,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .unwrap_or_else(|e| panic!("arc: {e}"));
        let other_generatrix_start = Point3::new(-radius, 0.0, 0.0);
        let other_generatrix_end = Point3::new(-radius, 0.0, h);

        let bottom_curve = s.curves.add(Box::new(bottom_rim));
        let bottom_edge = s.edges.add(Edge::new(
            0,
            0,
            1,
            bottom_curve,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let other_gen_edge = s.add_line_edge(other_generatrix_end, other_generatrix_start);
        let top_curve = s.curves.add(Box::new(top_rim));
        let top_edge = s.edges.add(Edge::new(
            0,
            0,
            1,
            top_curve,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));

        let mut fillet_loop = Loop::new(1, LoopType::Outer);
        fillet_loop.add_edge(bottom_edge, true);
        fillet_loop.add_edge(other_gen_edge, true);
        fillet_loop.add_edge(top_edge, true);
        fillet_loop.add_edge(shared_edge, true);
        let fillet_outer_loop = s.loops.add(fillet_loop);
        let fillet_face = s.faces.add(Face::new(
            1,
            fillet_surface,
            fillet_outer_loop,
            FaceOrientation::Forward,
        ));

        let face_ids = [wall_face, fillet_face];
        let (model, solid_id) =
            model_with_solid(s.surfaces, s.faces, s.loops, s.edges, s.curves, &face_ids);
        (model, solid_id, fillet_face)
    }

    /// Two flat walls meeting DIRECTLY at a concave (reentrant) edge, no
    /// fillet -- the sharp-corner headline fixture.
    fn sharp_concave_corner_fixture(
        x_max: f64,
        y_max: f64,
        h: f64,
    ) -> (BRepModel, SolidId, EdgeId) {
        let mut s = Stores::new();

        let shared = s.add_line_edge(Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 0.0, h));

        let wall_a_plane = Plane::from_point_normal(Point3::new(0.0, 0.0, 0.0), Vector3::X)
            .unwrap_or_else(|e| panic!("plane: {e}"));
        let wall_a_surface = s.surfaces.add(Box::new(wall_a_plane));
        let a1 = s.add_line_edge(Point3::new(0.0, 0.0, h), Point3::new(0.0, y_max, h));
        let a2 = s.add_line_edge(Point3::new(0.0, y_max, h), Point3::new(0.0, y_max, 0.0));
        let a3 = s.add_line_edge(Point3::new(0.0, y_max, 0.0), Point3::new(0.0, 0.0, 0.0));
        let mut wall_a_loop = Loop::new(0, LoopType::Outer);
        for e in [shared, a1, a2, a3] {
            wall_a_loop.add_edge(e, true);
        }
        let wall_a_outer = s.loops.add(wall_a_loop);
        let wall_a = s.faces.add(Face::new(
            0,
            wall_a_surface,
            wall_a_outer,
            FaceOrientation::Forward,
        ));

        let wall_b_plane = Plane::from_point_normal(Point3::new(0.0, 0.0, 0.0), Vector3::Y)
            .unwrap_or_else(|e| panic!("plane: {e}"));
        let wall_b_surface = s.surfaces.add(Box::new(wall_b_plane));
        let b1 = s.add_line_edge(Point3::new(0.0, 0.0, 0.0), Point3::new(x_max, 0.0, 0.0));
        let b2 = s.add_line_edge(Point3::new(x_max, 0.0, 0.0), Point3::new(x_max, 0.0, h));
        let b3 = s.add_line_edge(Point3::new(x_max, 0.0, h), Point3::new(0.0, 0.0, h));
        let mut wall_b_loop = Loop::new(1, LoopType::Outer);
        for e in [b1, b2, b3, shared] {
            wall_b_loop.add_edge(e, true);
        }
        let wall_b_outer = s.loops.add(wall_b_loop);
        let wall_b = s.faces.add(Face::new(
            1,
            wall_b_surface,
            wall_b_outer,
            FaceOrientation::Forward,
        ));

        let face_ids = [wall_a, wall_b];
        let (model, solid_id) =
            model_with_solid(s.surfaces, s.faces, s.loops, s.edges, s.curves, &face_ids);
        (model, solid_id, shared)
    }

    /// A `Torus` analogue of the concave fixture, using the module docs'
    /// `v=pi/2` derivation directly: at `v=pi/2` the torus's natural normal
    /// is exactly `+axis` for EVERY `u`, so a full-`u` band from `v=0` to
    /// `v=pi/2`, `Backward`-oriented, shares its `v=pi/2` boundary circle
    /// (radius = major_radius, height = minor_radius) EXACTLY with a flat
    /// deck plane of matching normal `-axis` (Backward flips `+axis` to
    /// `-axis`) -- exact tangency, no generatrix lines needed since both
    /// boundaries are full circles.
    fn concave_torus_fillet_fixture(
        major_radius: f64,
        minor_radius: f64,
    ) -> (BRepModel, SolidId, FaceId) {
        let mut s = Stores::new();
        let center = Point3::new(0.0, 0.0, 0.0);
        let axis = Vector3::Z;

        // Shared boundary: circle radius = major_radius at z = minor_radius
        // (the torus's v=pi/2 rim).
        let shared = s.add_arc_edge(Point3::new(0.0, 0.0, minor_radius), axis, major_radius);

        let torus = Torus::new(center, axis, major_radius, minor_radius)
            .unwrap_or_else(|e| panic!("torus: {e}"));
        let torus_surface = s.surfaces.add(Box::new(torus));
        // v=0 rim: circle radius = major_radius + minor_radius at z=0.
        let v0_rim = s.add_arc_edge(
            Point3::new(0.0, 0.0, 0.0),
            axis,
            major_radius + minor_radius,
        );
        let mut torus_loop = Loop::new(0, LoopType::Outer);
        torus_loop.add_edge(v0_rim, true);
        torus_loop.add_edge(shared, true);
        let torus_outer = s.loops.add(torus_loop);
        let torus_face = s.faces.add(Face::new(
            0,
            torus_surface,
            torus_outer,
            FaceOrientation::Backward,
        ));

        // Deck: flat plane at z = minor_radius, outward normal -Z (matches
        // the Backward torus's own outward normal there), a full disk of
        // radius major_radius sharing the SAME edge id.
        let deck_plane = Plane::from_point_normal(
            Point3::new(0.0, 0.0, minor_radius),
            Vector3::new(0.0, 0.0, -1.0),
        )
        .unwrap_or_else(|e| panic!("plane: {e}"));
        let deck_surface = s.surfaces.add(Box::new(deck_plane));
        let mut deck_loop = Loop::new(1, LoopType::Outer);
        deck_loop.add_edge(shared, true);
        let deck_outer = s.loops.add(deck_loop);
        let deck_face = s.faces.add(Face::new(
            1,
            deck_surface,
            deck_outer,
            FaceOrientation::Forward,
        ));

        let face_ids = [torus_face, deck_face];
        let (model, solid_id) =
            model_with_solid(s.surfaces, s.faces, s.loops, s.edges, s.curves, &face_ids);
        (model, solid_id, torus_face)
    }

    // ----- Hand-computed exact cases -----

    #[test]
    fn concave_cylindrical_fillet_reports_exact_radius() {
        let (model, solid_id, fillet_face) = concave_cylinder_fillet_fixture(2.5, 20.0, 5.0);
        let outcome = blend_radius(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert_eq!(
            outcome.blends.len(),
            1,
            "expected exactly one blend: {:?}",
            outcome.blends
        );
        assert_eq!(outcome.blends[0].face, fillet_face);
        assert!(
            (outcome.blends[0].radius.value - 2.5).abs() < 1e-9,
            "radius = {}",
            outcome.blends[0].radius.value
        );
        assert!(outcome.unverifiable.is_empty());
    }

    #[test]
    fn concave_toroidal_fillet_reports_exact_minor_radius() {
        let (model, solid_id, torus_face) = concave_torus_fillet_fixture(10.0, 2.0);
        let outcome = blend_radius(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert_eq!(
            outcome.blends.len(),
            1,
            "expected exactly one blend: {:?}",
            outcome.blends
        );
        assert_eq!(outcome.blends[0].face, torus_face);
        assert!(
            (outcome.blends[0].radius.value - 2.0).abs() < 1e-6,
            "radius = {}",
            outcome.blends[0].radius.value
        );
        assert!(
            outcome.unverifiable.is_empty(),
            "the flat deck tangent to the torus must not itself read as a refused blend: {:?}",
            outcome.unverifiable
        );
    }

    #[test]
    fn sharp_concave_corner_reports_radius_zero() {
        let (model, solid_id, shared_edge) = sharp_concave_corner_fixture(20.0, 20.0, 5.0);
        let outcome = blend_radius(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.blends.is_empty(),
            "no cylinder/torus faces present: {:?}",
            outcome.blends
        );
        assert_eq!(
            outcome.sharp_corners.len(),
            1,
            "expected exactly one sharp corner: {:?}",
            outcome.sharp_corners
        );
        assert_eq!(outcome.sharp_corners[0].edge, shared_edge);
        assert_eq!(outcome.sharp_corners[0].radius.value, 0.0);
    }

    // ----- THE ANTI-FABRICATION HEADLINE -----

    /// A CONVEX outer fillet -- genuinely tangent, exactly like the concave
    /// one, differing ONLY in orientation and which side the fillet center
    /// sits on -- must NEVER be reported as an internal corner radius.
    #[test]
    fn convex_outer_fillet_is_never_reported_as_internal_radius() {
        let (model, solid_id, fillet_face) = convex_cylinder_fillet_fixture(2.5, -20.0, 5.0);
        let outcome = blend_radius(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.blends.iter().all(|b| b.face != fillet_face),
            "a convex fillet must never appear as a blend: {:?}",
            outcome.blends
        );
        assert!(
            outcome.unverifiable.iter().all(|u| u.face != fillet_face),
            "a convex fillet is simply not a candidate -- not even a refusal: {:?}",
            outcome.unverifiable
        );
        assert!(outcome.blends.is_empty());
    }

    /// Mutation proof, raw before/after: drop the `Backward`-only guard
    /// (accept ANY tangent Cylinder/Torus face regardless of orientation --
    /// exactly what "concavity assumed, not derived" would do) and show the
    /// convex fixture WOULD be fabricated into a blend. Then confirm the
    /// real, orientation-gated `blend_radius` does not.
    #[test]
    fn mutation_proof_dropping_the_orientation_guard_would_fabricate_a_convex_blend() {
        let (model, solid_id, fillet_face) = convex_cylinder_fillet_fixture(2.5, -20.0, 5.0);

        // BEFORE (mutant): tangency-only gate, no concavity check at all.
        let mutant_would_report = face_has_tangent_boundary_edge(&model, fillet_face);
        assert!(
            mutant_would_report,
            "the mutant's tangency-only predicate must actually fire on the convex fillet, or \
             this test proves nothing about the orientation guard specifically"
        );
        let face = model
            .faces
            .get(fillet_face)
            .unwrap_or_else(|| panic!("fillet face resolves"));
        assert_eq!(
            face.orientation,
            FaceOrientation::Forward,
            "sanity: the convex fillet is Forward-oriented, which is exactly what the real \
             `face.orientation != FaceOrientation::Backward` guard must exclude"
        );

        // AFTER (real production path): the actual analyzer, which DOES
        // gate on concavity.
        let outcome = blend_radius(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));
        assert!(
            outcome.blends.iter().all(|b| b.face != fillet_face),
            "production path must never report the convex fillet even though the \
             orientation-free mutant predicate says it qualifies: {:?}",
            outcome.blends
        );
    }

    // ----- Refusal flow-through: freeform blend -----

    /// A flat NURBS patch acting as a genuinely tangent blend transition
    /// (shares its `v=0` boundary line, EXACTLY coplanar and therefore
    /// EXACTLY tangent, with a flat deck of the same normal) must read
    /// `Unverifiable{UnsupportedSurface}` naming NURBS, never a fabricated
    /// radius.
    #[test]
    fn tangent_freeform_blend_is_unverifiable_naming_the_kind() {
        let mut s = Stores::new();

        // Bilinear flat NURBS patch: control points at z=0, du ~ +X,
        // dv ~ +Y -- natural normal +Z (hand-derived: du x dv = X x Y = Z).
        let nurbs = crate::math::nurbs::NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .unwrap_or_else(|e| panic!("trivial flat NURBS patch fixture: {e}"));
        let nurbs_surface_id =
            s.surfaces
                .add(Box::new(crate::primitives::surface::GeneralNurbsSurface {
                    nurbs,
                }));

        // Shared boundary: the v=0 edge of the patch, the line from (0,0,0)
        // to (1,0,0) -- exactly on both the patch (its own v=0 isoline) and
        // the flat deck below.
        let shared = s.add_line_edge(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0));
        let mut nurbs_loop = Loop::new(0, LoopType::Outer);
        nurbs_loop.add_edge(shared, true);
        let nurbs_outer = s.loops.add(nurbs_loop);
        let nurbs_face = s.faces.add(Face::new(
            0,
            nurbs_surface_id,
            nurbs_outer,
            FaceOrientation::Forward,
        ));

        // Deck: flat plane z=0, normal +Z (matching), spanning
        // y in [-1, 0], x in [0, 1] -- shares the SAME edge.
        let deck_plane = Plane::from_point_normal(Point3::new(0.0, 0.0, 0.0), Vector3::Z)
            .unwrap_or_else(|e| panic!("plane: {e}"));
        let deck_surface = s.surfaces.add(Box::new(deck_plane));
        let d1 = s.add_line_edge(Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, -1.0, 0.0));
        let d2 = s.add_line_edge(Point3::new(1.0, -1.0, 0.0), Point3::new(0.0, -1.0, 0.0));
        let d3 = s.add_line_edge(Point3::new(0.0, -1.0, 0.0), Point3::new(0.0, 0.0, 0.0));
        let mut deck_loop = Loop::new(1, LoopType::Outer);
        for e in [shared, d1, d2, d3] {
            deck_loop.add_edge(e, true);
        }
        let deck_outer = s.loops.add(deck_loop);
        let deck_face = s.faces.add(Face::new(
            1,
            deck_surface,
            deck_outer,
            FaceOrientation::Forward,
        ));

        let face_ids = [nurbs_face, deck_face];
        let (model, solid_id) =
            model_with_solid(s.surfaces, s.faces, s.loops, s.edges, s.curves, &face_ids);

        let outcome = blend_radius(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(outcome.blends.is_empty());
        assert_eq!(
            outcome.unverifiable.len(),
            1,
            "expected exactly one refusal: {:?}",
            outcome.unverifiable
        );
        assert_eq!(outcome.unverifiable[0].face, nurbs_face);
        match &outcome.unverifiable[0].reason {
            UnverifiableReason::UnsupportedSurface { surface_type, .. } => {
                assert_eq!(*surface_type, SurfaceKind::Nurbs)
            }
            other => panic!("expected UnsupportedSurface, got {other:?}"),
        }
    }

    // ----- Bore reuse: a full concave cylinder without tangency is not a blend -----

    /// A through-bore's wall (S4's own headline candidate: `Backward`,
    /// full concave cylinder) has NO shared-edge tangency to any neighbour
    /// in the bore fixture (its rims and the plate's inner-loop rims are
    /// built as SEPARATE edge ids, `bore.rs`'s own fixture convention) --
    /// it must not be fabricated into a blend either.
    #[test]
    fn bore_wall_without_tangency_is_not_reported_as_a_blend() {
        use crate::dfm::analyzers::bore::fixtures::plate_with_through_bore;
        let (model, solid_id, bore_face) = plate_with_through_bore(20.0, 20.0, 10.0, 3.0);

        let outcome = blend_radius(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.blends.iter().all(|b| b.face != bore_face),
            "a bore wall must never be reported as a blend: {:?}",
            outcome.blends
        );
        assert!(
            outcome.unverifiable.iter().all(|u| u.face != bore_face),
            "a bore wall is simply not a candidate here: {:?}",
            outcome.unverifiable
        );
    }

    /// Malformed input: a dangling `solid_id` is an `Err`, never a
    /// refusal value (spec §4).
    #[test]
    fn dangling_solid_ref_is_an_error_not_a_refusal() {
        let model = BRepModel::new();
        let result = blend_radius(&model, 999);
        assert!(matches!(
            result,
            Err(DfmError::DanglingSolidRef { solid: 999 })
        ));
    }
}
