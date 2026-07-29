//! `bore_metrics` — per-bore diameter, trimmed axial depth, through-vs-blind,
//! and aspect ratio (spec §3.1 / §3.2 `fdm.min_bore`).
//!
//! ## Reusing the material-side rule (do not re-derive it)
//!
//! Distinguishing a BORE (a cavity wall) from a BOSS (an external post) or
//! the part's own outer diameter is exactly the "kernel can lie" defect
//! `drawing/hole_table.rs`'s module doc names directly: unfiltered
//! cylindrical-face records "put the part's silhouette in the hole table."
//! [`crate::readable::bore_face_ids`] already implements the correct rule —
//! a cylindrical face is a bore iff its outward normal points TOWARD the
//! axis (`FaceOrientation::Backward`) — and this analyzer calls it
//! VERBATIM as its candidate filter rather than re-deriving the same
//! Cylinder+orientation check a second time.
//!
//! ## Why this analyzer's contract is `(model, solid_id)`, not `faces:
//! &[FaceId]` + bare stores
//!
//! Every other S2/S3 analyzer ([`crate::dfm::analyzers::face_orientation_field`],
//! [`crate::dfm::analyzers::pair_thickness`]) answers a purely FACE-LOCAL
//! question, so `packs/mod.rs`'s own docs could push "enumerate every face
//! of the solid" onto the caller as one line, with no `Solid`/`ShellStore`
//! dependency needed inside the analyzer. THRU-vs-blind is not face-local:
//! "does this hole go all the way through the part?" can only be answered
//! by comparing the bore wall's own trimmed extent against the SOLID's own
//! extent along the same axis — which requires walking every face of the
//! solid, not just the bore wall. Threading "the solid's extent" through
//! as yet another caller-supplied parameter would let a caller pass a
//! mismatched or partial face list and get a silently wrong THRU/blind bit
//! with no signal that anything was wrong — precisely the defect class
//! this subsystem exists to kill. `(model, solid_id)` makes that
//! unconstructable: `bore_face_ids` and this analyzer's own solid-extent
//! walk both read the SAME solid, directly, from the model being asked
//! about.
//!
//! ## Two distinct axial extents — do not conflate them
//!
//! - **The bore wall face's own trimmed extent** reuses
//!   [`crate::dfm::analyzers::thickness::axial_extent`] EXACTLY (promoted
//!   `pub(super)` there for this reuse) — a Line edge contributes its
//!   endpoints, an axis-perpendicular rim Arc contributes its center,
//!   anything else (inner loop present, non-rim boundary) refuses. That
//!   analyzer's inner-loop refusal is correct for a cylindrical wall face
//!   (which never legitimately has one).
//! - **The solid's own extent along the bore axis** ([`solid_axial_extent`])
//!   is a SEPARATE, wider-scoped walk: it visits every face of the solid
//!   and BOTH the outer loop AND every inner loop, because a plate's flat
//!   top/bottom faces carry the bore's own rim as an INNER loop — reusing
//!   `thickness::axial_extent` (which refuses outright on any inner loop)
//!   for this would refuse the exact plate-with-a-through-hole fixture
//!   this analyzer exists to handle. The looser SCOPE (every loop of every
//!   face) does not loosen the EXACTNESS discipline: every boundary
//!   element still contributes only a Line endpoint or an axis-
//!   perpendicular rim Arc's center; the walk refuses (returns `None`) the
//!   instant it meets anything else (a NURBS/BSpline boundary curve, a
//!   non-perpendicular arc) rather than silently dropping that face from
//!   the bound — a dropped face could only ever make the reported extent
//!   too SHORT, mislabelling a THRU hole as blind.
//!
//! ## Refusal semantics (spec §3.1's "non-cylindrical holes" column)
//!
//! With `bore_face_ids` as the candidate filter, the trichotomy is:
//!
//! - **Forward-oriented cylinder** (boss / the part's own OD): NOT in the
//!   candidate set at all → no entry in [`BoreMetricsOutcome`], neither a
//!   bore nor a refusal. This is the anti-fabrication behavior the
//!   headline test proves.
//! - **Plane / Cone / Sphere / Torus / NURBS faces**: also not in the
//!   candidate set → no entry. Flagging every non-cylindrical face
//!   `Unverifiable` would make every real part read `Inconclusive` for
//!   `fdm.min_bore` — worse than useless, and not what the spec's
//!   analyzer-support table describes (it names bores' own refusal
//!   modes, not "every other face on the part").
//! - **A genuine bore (Backward-oriented cylinder) whose trimmed extent
//!   or whose solid-extent bound cannot be derived exactly**:
//!   [`UnverifiableReason::UnsupportedTopology`], naming the face.
//!
//! ## Named v1 non-goal (absence ≠ oversight)
//!
//! "Non-cylindrical holes" (a hex-broached socket, a cone-walled
//! countersink) are invisible to `bore_face_ids` and therefore to this
//! analyzer in v1 — there is no non-cylindrical hole-recognition machinery
//! anywhere in this tree to reuse, and inventing one here would be exactly
//! the kind of untested, unreviewed detector the spec's analyzer table
//! does not ask for. A cone-walled countersink simply does not appear in
//! [`BoreMetricsOutcome`] at all (silently excluded, same as any other
//! non-cylindrical face) — not a fabricated pass, but also not a proven
//! refusal; it is invisible in exactly the same sense a NURBS blend face
//! is invisible to `pair_thickness`'s candidate classification.

use crate::dfm::analyzers::orientation::RIM_PERPENDICULAR_TOL;
use crate::dfm::analyzers::thickness::axial_extent;
use crate::dfm::report::{
    Derivation, DfmError, DfmValue, FaceRef, SurfaceKind, UnverifiableReason,
};
use crate::math::{Point3, Vector3};
use crate::primitives::curve::{Arc, Line};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Cylinder;
use crate::primitives::topology_builder::BRepModel;

/// The 0.01 mm THRU-vs-blind equality tolerance, reused (not reinvented)
/// from [`crate::drawing::hole_table::build_hole_table`]'s own documented
/// convention: "bore length equals the part extent within 0.01 mm".
const THRU_EQUALITY_TOL_MM: f64 = 0.01;

/// One proven bore (spec §3.1 `bore_metrics`): a face
/// [`crate::readable::bore_face_ids`] classifies as a concave cylindrical
/// wall, with every reported number carrying its [`Derivation`].
#[derive(Debug, Clone, PartialEq)]
pub struct BoreRecord {
    /// The bore wall face id.
    pub face: FaceRef,
    /// Exact diameter (`2 × radius`).
    pub diameter: DfmValue,
    /// Exact axial depth over the wall face's TRIMMED extent.
    pub depth: DfmValue,
    /// Whether the bore's own trimmed extent equals the solid's extent
    /// along the same axis within [`THRU_EQUALITY_TOL_MM`].
    pub is_through: bool,
    /// `depth / diameter`.
    pub aspect_ratio: DfmValue,
}

/// A proven bore candidate whose depth/thru-blind could not be determined
/// exactly (module docs' refusal semantics) — not itself a Pass/Violation
/// decision, folded by a rule (`fdm.min_bore`).
#[derive(Debug, Clone, PartialEq)]
pub struct UnverifiableBore {
    pub face: FaceRef,
    pub reason: UnverifiableReason,
}

/// The full result of one `bore_metrics` call (spec §4: a refusal is a
/// VALUE, never an error — [`DfmError`] is reserved for malformed input:
/// a dangling solid reference).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoreMetricsOutcome {
    pub bores: Vec<BoreRecord>,
    pub unverifiable: Vec<UnverifiableBore>,
}

/// Every face belonging to `solid_id` (outer shell + every inner shell) —
/// the SAME solid→shell→face enumeration
/// [`crate::readable::bore_face_ids`] performs internally, needed here a
/// second time to walk EVERY face (not just the bore candidates) for
/// [`solid_axial_extent`]. This is plain topology traversal, not the
/// material-side rule — the same pattern
/// `readable::dimensions::world_aabb` and `Solid::compute_stats` already
/// use elsewhere in the tree.
fn solid_faces(model: &BRepModel, solid_id: SolidId) -> Option<Vec<FaceId>> {
    let solid = model.solids.get(solid_id)?;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut faces = Vec::new();
    for sh in shells {
        if let Some(shell) = model.shells.get(sh) {
            faces.extend(shell.faces.iter().copied());
        }
    }
    Some(faces)
}

/// The solid's own exact extent along `axis` (through `axis_point`),
/// walking every face's OUTER and INNER loops (module docs: the wider,
/// laxer counterpart of `thickness::axial_extent`). Refuses (`None`) the
/// instant any boundary edge is neither a `Line` nor an axis-perpendicular
/// rim `Arc` — never silently drops a face from the bound (module docs: a
/// dropped face can only make the reported extent too short).
fn solid_axial_extent(
    model: &BRepModel,
    faces: &[FaceId],
    axis_point: Point3,
    axis_dir: Vector3,
) -> Option<(f64, f64)> {
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    let mut touched = false;

    for &face_id in faces {
        let face = model.faces.get(face_id)?;
        let mut loops = vec![face.outer_loop];
        loops.extend_from_slice(&face.inner_loops);
        for lid in loops {
            let Some(lp) = model.loops.get(lid) else {
                continue; // an empty/placeholder loop contributes nothing
            };
            for &eid in &lp.edges {
                let edge = model.edges.get(eid)?;
                let curve = model.curves.get(edge.curve_id)?;
                if let Some(line) = curve.as_any().downcast_ref::<Line>() {
                    for p in [line.start, line.end] {
                        let v = (p - axis_point).dot(&axis_dir);
                        v_min = v_min.min(v);
                        v_max = v_max.max(v);
                        touched = true;
                    }
                } else if let Some(arc) = curve.as_any().downcast_ref::<Arc>() {
                    let alignment = arc.normal.dot(&axis_dir);
                    if (alignment.abs() - 1.0).abs() > RIM_PERPENDICULAR_TOL {
                        return None; // not a rim relative to this axis
                    }
                    let v = (arc.center - axis_point).dot(&axis_dir);
                    v_min = v_min.min(v);
                    v_max = v_max.max(v);
                    touched = true;
                } else {
                    return None; // unsupported boundary curve kind
                }
            }
        }
    }

    if touched && v_min.is_finite() && v_max.is_finite() {
        Some((v_min, v_max))
    } else {
        None
    }
}

/// `bore_metrics` (spec §3.1 / §3.2 `fdm.min_bore`'s analyzer): every
/// concave-cylindrical bore of `solid_id`, with exact diameter, trimmed
/// depth, thru/blind, and aspect ratio — or an honest per-bore refusal
/// (module docs). See the module docs for why this analyzer's contract is
/// `(model, solid_id)` rather than `faces: &[FaceId]` + bare stores.
///
/// Returns `Err` only for a dangling `solid_id` (spec §4: malformed input,
/// never a legitimate analyzer outcome).
pub fn bore_metrics(model: &BRepModel, solid_id: SolidId) -> Result<BoreMetricsOutcome, DfmError> {
    let all_faces =
        solid_faces(model, solid_id).ok_or(DfmError::DanglingSolidRef { solid: solid_id })?;

    // THE MATERIAL-SIDE RULE, REUSED VERBATIM -- not re-derived (module
    // docs). `bore_face_ids` returns `HashSet<u32>`; `FaceId` IS `u32`, so
    // no conversion layer is needed.
    let mut candidate_faces: Vec<FaceId> = crate::readable::bore_face_ids(model, solid_id)
        .into_iter()
        .collect();
    candidate_faces.sort_unstable(); // deterministic order, not HashSet iteration order

    let mut bores = Vec::new();
    let mut unverifiable = Vec::new();

    for face_id in candidate_faces {
        let Some(face) = model.faces.get(face_id) else {
            continue; // unreachable: bore_face_ids only returns resolved faces
        };
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue; // unreachable: bore_face_ids only returns resolved faces
        };
        let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() else {
            continue; // unreachable: bore_face_ids only returns Cylinder faces
        };

        let diameter_value = 2.0 * cyl.radius;

        let Some((wall_min, wall_max)) = axial_extent(
            face,
            cyl.origin,
            cyl.axis,
            &model.loops,
            &model.edges,
            &model.curves,
        ) else {
            unverifiable.push(UnverifiableBore {
                face: face_id,
                reason: UnverifiableReason::UnsupportedTopology {
                    detail: format!(
                        "bore face {face_id}: trimmed axial extent could not be derived from \
                         its outer-loop boundary (non-Line, non-axis-perpendicular-rim edge)"
                    ),
                },
            });
            continue;
        };
        let depth_value = wall_max - wall_min;

        let Some((solid_min, solid_max)) =
            solid_axial_extent(model, &all_faces, cyl.origin, cyl.axis)
        else {
            unverifiable.push(UnverifiableBore {
                face: face_id,
                reason: UnverifiableReason::UnsupportedTopology {
                    detail: format!(
                        "bore face {face_id}: solid's own extent along the bore axis could not \
                         be derived exactly (a non-Line, non-axis-perpendicular-rim boundary \
                         edge exists on the solid)"
                    ),
                },
            });
            continue;
        };
        let part_extent_along_axis = solid_max - solid_min;
        let is_through = (depth_value - part_extent_along_axis).abs() <= THRU_EQUALITY_TOL_MM;

        let aspect_value = depth_value / diameter_value;

        bores.push(BoreRecord {
            face: face_id,
            diameter: DfmValue::new(
                diameter_value,
                Derivation::Analytic {
                    surface_type: SurfaceKind::Cylinder,
                    method: "bore diameter: 2x cylinder radius (bore_face_ids concave-face rule)"
                        .to_string(),
                },
            ),
            depth: DfmValue::new(
                depth_value,
                Derivation::Analytic {
                    surface_type: SurfaceKind::Cylinder,
                    method: "bore depth: trimmed axial extent of the bore wall face".to_string(),
                },
            ),
            is_through,
            aspect_ratio: DfmValue::new(
                aspect_value,
                Derivation::Analytic {
                    surface_type: SurfaceKind::Cylinder,
                    method: "bore aspect ratio: depth / diameter".to_string(),
                },
            ),
        });
    }

    Ok(BoreMetricsOutcome {
        bores,
        unverifiable,
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Bare-store solid fixtures (no boolean/extrude — the same
    //! KNOWN_REDS-avoidance convention `thickness.rs`'s own test module
    //! documents). `pub(crate)` so `packs::fdm`'s full-pack integration
    //! test can reuse the through-bore fixture instead of re-deriving the
    //! same hand-built plate geometry a second time.

    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::{Arc, CurveStore, Line, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation, EdgeStore};
    use crate::primitives::face::{Face, FaceId, FaceOrientation, FaceStore};
    use crate::primitives::r#loop::{Loop, LoopStore, LoopType};
    use crate::primitives::shell::{Shell, ShellType};
    use crate::primitives::solid::{Solid, SolidId};
    use crate::primitives::surface::{Cylinder, Plane, SurfaceStore};
    use crate::primitives::topology_builder::BRepModel;

    /// Bundled bare stores under construction, mirroring `thickness.rs`'s
    /// own `Stores` test helper (independently, per this module's own
    /// geometry needs — a plate's rectangular boundary + circular inner
    /// loop is a different shape vocabulary than `thickness.rs`'s wall
    /// pairs).
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

        /// A rectangular PLANE face `[0, lx] x [0, ly]` at `z = z_plane`
        /// with outward normal `normal` (unit) and no inner loop.
        fn add_plate_face(&mut self, z_plane: f64, normal: Vector3, lx: f64, ly: f64) -> FaceId {
            let plane = Plane::from_point_normal(Point3::new(0.0, 0.0, z_plane), normal)
                .unwrap_or_else(|e| panic!("valid plane fixture: {e}"));
            let surface_id = self.surfaces.add(Box::new(plane));

            let corners = [
                Point3::new(0.0, 0.0, z_plane),
                Point3::new(lx, 0.0, z_plane),
                Point3::new(lx, ly, z_plane),
                Point3::new(0.0, ly, z_plane),
            ];
            let mut loop_ = Loop::new(0, LoopType::Outer);
            for i in 0..4 {
                let (start, end) = (corners[i], corners[(i + 1) % 4]);
                let curve_id = self.curves.add(Box::new(Line::new(start, end)));
                let edge = Edge::new(
                    0,
                    0,
                    1,
                    curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::unit(),
                );
                let edge_id = self.edges.add(edge);
                loop_.add_edge(edge_id, true);
            }
            let outer_loop = self.loops.add(loop_);
            let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
            self.faces.add(face)
        }

        /// Cut a circular inner loop of `radius` centred at `(cx, cy)` in
        /// the SAME plane the target face already lives in (`z_plane`),
        /// into `face_id`. The rim's arc `normal` is `+/-Z`, matching the
        /// bore axis exactly — geometrically the same "rim perpendicular
        /// to the axis" shape a cylinder's own end cap has, since the
        /// hole is cut straight through a flat plate.
        fn add_inner_hole_loop(
            &mut self,
            face_id: FaceId,
            cx: f64,
            cy: f64,
            z_plane: f64,
            radius: f64,
        ) {
            let rim = Arc::circle(Point3::new(cx, cy, z_plane), Vector3::Z, radius)
                .unwrap_or_else(|e| panic!("valid rim arc fixture: {e}"));
            let curve_id = self.curves.add(Box::new(rim));
            let edge = Edge::new(
                0,
                0,
                1,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            );
            let edge_id = self.edges.add(edge);
            let mut loop_ = Loop::new(0, LoopType::Inner);
            loop_.add_edge(edge_id, true);
            let inner_loop = self.loops.add(loop_);
            if let Some(face) = self.faces.get_mut(face_id) {
                face.add_inner_loop(inner_loop);
            }
        }

        /// A CYLINDER wall face (bottom rim at `v_bottom`, top rim at
        /// `v_top`, both full circles — mirrors `thickness.rs`'s
        /// `add_full_cylinder_face`) with the given orientation.
        /// `orientation = Backward` -> a bore wall (material outside,
        /// void toward axis, [`crate::readable::bore_face_ids`]'s
        /// concave rule); `Forward` -> a boss/OD (material inside).
        #[allow(clippy::too_many_arguments)]
        fn add_full_cylinder_face(
            &mut self,
            origin: Point3,
            axis: Vector3,
            radius: f64,
            v_bottom: f64,
            v_top: f64,
            orientation: FaceOrientation,
        ) -> FaceId {
            let cylinder = Cylinder::new(origin, axis, radius).unwrap_or_else(|e| panic!("{e}"));
            let surface_id = self.surfaces.add(Box::new(cylinder));

            let bottom = Arc::circle(origin + axis * v_bottom, axis, radius)
                .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));
            let top = Arc::circle(origin + axis * v_top, axis, radius)
                .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));

            let mut loop_ = Loop::new(0, LoopType::Outer);
            for curve in [
                Box::new(bottom) as Box<dyn crate::primitives::curve::Curve>,
                Box::new(top),
            ] {
                let curve_id = self.curves.add(curve);
                let edge = Edge::new(
                    0,
                    0,
                    1,
                    curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::unit(),
                );
                let edge_id = self.edges.add(edge);
                loop_.add_edge(edge_id, true);
            }
            let outer_loop = self.loops.add(loop_);
            let face = Face::new(0, surface_id, outer_loop, orientation);
            self.faces.add(face)
        }

        /// A CYLINDER wall face whose outer loop carries ONE properly
        /// axis-perpendicular rim and ONE deliberately SKEWED rim (its
        /// arc plane tilted off-axis) — an honest refusal fixture:
        /// `thickness::axial_extent` (reused by `bore_metrics`) cannot
        /// trust a non-rim boundary curve, so this face's trimmed extent
        /// is unreconstructable in closed form.
        fn add_cylinder_face_with_skewed_rim(
            &mut self,
            origin: Point3,
            axis: Vector3,
            radius: f64,
            v_bottom: f64,
            v_top: f64,
            orientation: FaceOrientation,
        ) -> FaceId {
            let cylinder = Cylinder::new(origin, axis, radius).unwrap_or_else(|e| panic!("{e}"));
            let surface_id = self.surfaces.add(Box::new(cylinder));

            let good_rim = Arc::circle(origin + axis * v_bottom, axis, radius)
                .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));
            // Skewed rim: its plane's normal is 45 deg off the cylinder's
            // own axis, so `RIM_PERPENDICULAR_TOL` cannot classify it as
            // a real end cap -- axial_extent must refuse, not guess.
            let tilt = Vector3::new(axis.x + 1.0, axis.y, axis.z)
                .normalize()
                .unwrap_or(axis);
            let skewed_rim = Arc::circle(origin + axis * v_top, tilt, radius)
                .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));

            let mut loop_ = Loop::new(0, LoopType::Outer);
            for curve in [
                Box::new(good_rim) as Box<dyn crate::primitives::curve::Curve>,
                Box::new(skewed_rim),
            ] {
                let curve_id = self.curves.add(curve);
                let edge = Edge::new(
                    0,
                    0,
                    1,
                    curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::unit(),
                );
                let edge_id = self.edges.add(edge);
                loop_.add_edge(edge_id, true);
            }
            let outer_loop = self.loops.add(loop_);
            let face = Face::new(0, surface_id, outer_loop, orientation);
            self.faces.add(face)
        }
    }

    /// Wrap already-built bare stores + a face list into a fresh
    /// [`BRepModel`] with ONE solid (a single closed shell containing
    /// every given face) — the model-level plumbing `bore_metrics`
    /// needs (solid -> shell -> faces) on top of geometry already built
    /// the same bare-store way every other analyzer's tests build it.
    fn wrap_into_model(stores: Stores, face_ids: &[FaceId]) -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        model.surfaces = stores.surfaces;
        model.faces = stores.faces;
        model.loops = stores.loops;
        model.edges = stores.edges;
        model.curves = stores.curves;

        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_faces(face_ids);
        let shell_id = model.shells.add(shell);
        let solid = Solid::new(0, shell_id);
        let solid_id = model.solids.add(solid);
        (model, solid_id)
    }

    /// A box plate `lx x ly x lz` with a coaxial cylindrical hole of
    /// `radius`, drilled all the way through along +Z (z=0 to z=lz) —
    /// the headline THRU fixture. Returns `(model, solid_id, bore_face)`.
    pub(crate) fn plate_with_through_bore(
        lx: f64,
        ly: f64,
        lz: f64,
        radius: f64,
    ) -> (BRepModel, SolidId, FaceId) {
        let mut s = Stores::new();
        let cx = lx / 2.0;
        let cy = ly / 2.0;

        // 4 side walls (no holes -- the hole doesn't reach the sides).
        let side_x0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(-1.0, 0.0, 0.0),
        );
        let side_x1 = side_wall(
            &mut s,
            Point3::new(lx, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(1.0, 0.0, 0.0),
        );
        let side_y0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, -1.0, 0.0),
        );
        let side_y1 = side_wall(
            &mut s,
            Point3::new(0.0, ly, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, 1.0, 0.0),
        );

        // Top (z=lz, normal +Z) and bottom (z=0, normal -Z) faces, each
        // with the hole's circular rim as an INNER loop -- this is what
        // `solid_axial_extent` must walk (module docs) that
        // `thickness::axial_extent` deliberately cannot.
        let top = s.add_plate_face(lz, Vector3::Z, lx, ly);
        s.add_inner_hole_loop(top, cx, cy, lz, radius);
        let bottom = s.add_plate_face(0.0, Vector3::new(0.0, 0.0, -1.0), lx, ly);
        s.add_inner_hole_loop(bottom, cx, cy, 0.0, radius);

        // The bore wall itself: concave (Backward), full depth 0..lz.
        let bore = s.add_full_cylinder_face(
            Point3::new(cx, cy, 0.0),
            Vector3::Z,
            radius,
            0.0,
            lz,
            FaceOrientation::Backward,
        );

        let face_ids = [side_x0, side_x1, side_y0, side_y1, top, bottom, bore];
        let (model, solid_id) = wrap_into_model(s, &face_ids);
        (model, solid_id, bore)
    }

    /// Same plate, but the bore only reaches `depth` down from the top
    /// (`z in [lz - depth, lz]`) -- a blind pocket. The bottom face has
    /// NO inner loop (the hole never reaches it); only the top does.
    pub(crate) fn plate_with_blind_bore(
        lx: f64,
        ly: f64,
        lz: f64,
        radius: f64,
        depth: f64,
    ) -> (BRepModel, SolidId, FaceId) {
        let mut s = Stores::new();
        let cx = lx / 2.0;
        let cy = ly / 2.0;

        let side_x0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(-1.0, 0.0, 0.0),
        );
        let side_x1 = side_wall(
            &mut s,
            Point3::new(lx, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(1.0, 0.0, 0.0),
        );
        let side_y0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, -1.0, 0.0),
        );
        let side_y1 = side_wall(
            &mut s,
            Point3::new(0.0, ly, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, 1.0, 0.0),
        );

        let top = s.add_plate_face(lz, Vector3::Z, lx, ly);
        s.add_inner_hole_loop(top, cx, cy, lz, radius);
        let bottom = s.add_plate_face(0.0, Vector3::new(0.0, 0.0, -1.0), lx, ly);
        // no inner loop on the bottom face: the blind bore never reaches it.

        let bore = s.add_full_cylinder_face(
            Point3::new(cx, cy, 0.0),
            Vector3::Z,
            radius,
            lz - depth,
            lz,
            FaceOrientation::Backward,
        );

        let face_ids = [side_x0, side_x1, side_y0, side_y1, top, bottom, bore];
        let (model, solid_id) = wrap_into_model(s, &face_ids);
        (model, solid_id, bore)
    }

    /// A plate with an external cylindrical BOSS (post) standing on its
    /// top face -- Forward-oriented, must never be reported as a bore.
    pub(crate) fn plate_with_boss(
        lx: f64,
        ly: f64,
        lz: f64,
        radius: f64,
        boss_height: f64,
    ) -> (BRepModel, SolidId, FaceId) {
        let mut s = Stores::new();
        let cx = lx / 2.0;
        let cy = ly / 2.0;

        let side_x0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(-1.0, 0.0, 0.0),
        );
        let side_x1 = side_wall(
            &mut s,
            Point3::new(lx, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(1.0, 0.0, 0.0),
        );
        let side_y0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, -1.0, 0.0),
        );
        let side_y1 = side_wall(
            &mut s,
            Point3::new(0.0, ly, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, 1.0, 0.0),
        );
        let top = s.add_plate_face(lz, Vector3::Z, lx, ly);
        let bottom = s.add_plate_face(0.0, Vector3::new(0.0, 0.0, -1.0), lx, ly);

        let boss = s.add_full_cylinder_face(
            Point3::new(cx, cy, lz),
            Vector3::Z,
            radius,
            0.0,
            boss_height,
            FaceOrientation::Forward,
        );

        let face_ids = [side_x0, side_x1, side_y0, side_y1, top, bottom, boss];
        let (model, solid_id) = wrap_into_model(s, &face_ids);
        (model, solid_id, boss)
    }

    /// A bore whose wall face's outer loop has one good rim and one
    /// SKEWED rim -- must read `Unverifiable`, never a fabricated bore.
    pub(crate) fn plate_with_unreadable_bore(
        lx: f64,
        ly: f64,
        lz: f64,
        radius: f64,
    ) -> (BRepModel, SolidId, FaceId) {
        let mut s = Stores::new();
        let cx = lx / 2.0;
        let cy = ly / 2.0;

        let side_x0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(-1.0, 0.0, 0.0),
        );
        let side_x1 = side_wall(
            &mut s,
            Point3::new(lx, 0.0, 0.0),
            Vector3::Y,
            Vector3::Z,
            ly,
            lz,
            Vector3::new(1.0, 0.0, 0.0),
        );
        let side_y0 = side_wall(
            &mut s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, -1.0, 0.0),
        );
        let side_y1 = side_wall(
            &mut s,
            Point3::new(0.0, ly, 0.0),
            Vector3::X,
            Vector3::Z,
            lx,
            lz,
            Vector3::new(0.0, 1.0, 0.0),
        );
        let top = s.add_plate_face(lz, Vector3::Z, lx, ly);
        s.add_inner_hole_loop(top, cx, cy, lz, radius);
        let bottom = s.add_plate_face(0.0, Vector3::new(0.0, 0.0, -1.0), lx, ly);
        s.add_inner_hole_loop(bottom, cx, cy, 0.0, radius);

        let bore = s.add_cylinder_face_with_skewed_rim(
            Point3::new(cx, cy, 0.0),
            Vector3::Z,
            radius,
            0.0,
            lz,
            FaceOrientation::Backward,
        );

        let face_ids = [side_x0, side_x1, side_y0, side_y1, top, bottom, bore];
        let (model, solid_id) = wrap_into_model(s, &face_ids);
        (model, solid_id, bore)
    }

    /// A rectangular PLANE side wall spanning `u in [0, u_len]`, `v in
    /// [0, v_len]` in the plane through `origin` with in-plane axes
    /// `u_dir`/`v_dir`, outward normal `normal`.
    #[allow(clippy::too_many_arguments)]
    fn side_wall(
        s: &mut Stores,
        origin: Point3,
        u_dir: Vector3,
        v_dir: Vector3,
        u_len: f64,
        v_len: f64,
        normal: Vector3,
    ) -> FaceId {
        let plane = Plane::from_point_normal(origin, normal)
            .unwrap_or_else(|e| panic!("valid plane fixture: {e}"));
        let surface_id = s.surfaces.add(Box::new(plane));

        let corners = [
            origin,
            origin + u_dir * u_len,
            origin + u_dir * u_len + v_dir * v_len,
            origin + v_dir * v_len,
        ];
        let mut loop_ = Loop::new(0, LoopType::Outer);
        for i in 0..4 {
            let (start, end) = (corners[i], corners[(i + 1) % 4]);
            let curve_id = s.curves.add(Box::new(Line::new(start, end)));
            let edge = Edge::new(
                0,
                0,
                1,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            );
            let edge_id = s.edges.add(edge);
            loop_.add_edge(edge_id, true);
        }
        let outer_loop = s.loops.add(loop_);
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        s.faces.add(face)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{
        plate_with_blind_bore, plate_with_boss, plate_with_through_bore, plate_with_unreadable_bore,
    };
    use super::*;
    use crate::primitives::face::FaceOrientation;

    /// Hand-computed THRU case: a 20x20x10 plate with a centred Ø6 bore
    /// (radius 3) drilled all the way through reports the EXACT diameter
    /// and THRU.
    #[test]
    fn through_bore_reports_exact_diameter_and_thru() {
        let (model, solid_id, bore_face) = plate_with_through_bore(20.0, 20.0, 10.0, 3.0);
        let outcome = bore_metrics(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.unverifiable.is_empty(),
            "a plain through-bore must not refuse: {:?}",
            outcome.unverifiable
        );
        assert_eq!(
            outcome.bores.len(),
            1,
            "exactly one bore: {:?}",
            outcome.bores
        );
        let bore = &outcome.bores[0];
        assert_eq!(bore.face, bore_face);
        assert!(
            (bore.diameter.value - 6.0).abs() < 1e-9,
            "diameter = {}",
            bore.diameter.value
        );
        assert!(
            (bore.depth.value - 10.0).abs() < 1e-9,
            "depth = {}",
            bore.depth.value
        );
        assert!(bore.is_through, "a full-depth bore must read THRU");
        assert!(
            (bore.aspect_ratio.value - 10.0 / 6.0).abs() < 1e-9,
            "aspect = {}",
            bore.aspect_ratio.value
        );
    }

    /// Hand-computed BLIND case: same plate, bore only reaches 4mm down
    /// from the top (lz=10) -- exact depth 4.0, blind (not THRU).
    #[test]
    fn blind_bore_reports_exact_depth_and_blind() {
        let (model, solid_id, bore_face) = plate_with_blind_bore(20.0, 20.0, 10.0, 3.0, 4.0);
        let outcome = bore_metrics(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.unverifiable.is_empty(),
            "{:?}",
            outcome.unverifiable
        );
        assert_eq!(outcome.bores.len(), 1);
        let bore = &outcome.bores[0];
        assert_eq!(bore.face, bore_face);
        assert!(
            (bore.diameter.value - 6.0).abs() < 1e-9,
            "diameter = {}",
            bore.diameter.value
        );
        assert!(
            (bore.depth.value - 4.0).abs() < 1e-9,
            "depth = {}",
            bore.depth.value
        );
        assert!(!bore.is_through, "a 4mm-deep bore in a 10mm plate is blind");
    }

    // ----- THE ANTI-FABRICATION HEADLINE -----

    /// A BOSS (external cylindrical post) must NEVER be reported as a
    /// bore, even though it is a perfectly good full cylinder face --
    /// exactly the failure mode `bore_face_ids` exists to prevent
    /// (`drawing/hole_table.rs`'s module doc: unfiltered records "put the
    /// part's silhouette in the hole table").
    #[test]
    fn boss_is_never_reported_as_a_bore() {
        let (model, solid_id, boss_face) = plate_with_boss(20.0, 20.0, 10.0, 3.0, 5.0);
        let outcome = bore_metrics(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.bores.iter().all(|b| b.face != boss_face),
            "the boss face must never appear as a bore: {:?}",
            outcome.bores
        );
        assert!(
            outcome.unverifiable.iter().all(|u| u.face != boss_face),
            "the boss face must not even be flagged unverifiable -- it is simply not a \
             candidate: {:?}",
            outcome.unverifiable
        );
        // The plate has no real bore either: bores must be entirely empty.
        assert!(outcome.bores.is_empty(), "{:?}", outcome.bores);
    }

    /// Mutation proof, raw before/after: bypass the `bore_face_ids`
    /// filter (accept ANY cylindrical face regardless of orientation,
    /// exactly what a naive "cylinder = hole" heuristic would do) and
    /// show the boss face WOULD be fabricated into a bore. Then confirm
    /// the real, filtered `bore_metrics` path does not.
    #[test]
    fn mutation_proof_bypassing_bore_face_ids_would_fabricate_a_bore_from_the_boss() {
        let (model, solid_id, boss_face) = plate_with_boss(20.0, 20.0, 10.0, 3.0, 5.0);

        // BEFORE (mutant): treat every Cylinder-surfaced face in the
        // solid as a bore candidate, ignoring `face.orientation` entirely
        // -- the exact bug class `bore_face_ids` exists to prevent.
        let mutant_bore_candidates: Vec<FaceId> = {
            let solid = model
                .solids
                .get(solid_id)
                .unwrap_or_else(|| panic!("fixture solid resolves"));
            let mut shells = vec![solid.outer_shell];
            shells.extend_from_slice(&solid.inner_shells);
            let mut out = Vec::new();
            for sh in shells {
                if let Some(shell) = model.shells.get(sh) {
                    for &fid in &shell.faces {
                        if let Some(face) = model.faces.get(fid) {
                            if let Some(surface) = model.surfaces.get(face.surface_id) {
                                if surface.as_any().downcast_ref::<Cylinder>().is_some() {
                                    out.push(fid); // no orientation check at all
                                }
                            }
                        }
                    }
                }
            }
            out
        };
        assert!(
            mutant_bore_candidates.contains(&boss_face),
            "the mutant's orientation-free predicate must actually fire on the boss face, or \
             this test proves nothing about the orientation filter specifically"
        );
        let boss_face_orientation = model
            .faces
            .get(boss_face)
            .map(|f| f.orientation)
            .unwrap_or_else(|| panic!("boss face resolves"));
        assert_eq!(
            boss_face_orientation,
            FaceOrientation::Forward,
            "sanity: the boss is Forward-oriented (material inside), which is exactly what \
             `bore_face_ids` must exclude"
        );

        // AFTER (real production path): the actual analyzer, which DOES
        // filter through `bore_face_ids`.
        let outcome = bore_metrics(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));
        assert!(
            outcome.bores.iter().all(|b| b.face != boss_face),
            "production path must never report the boss face as a bore even though the \
             orientation-free mutant predicate says it qualifies: {:?}",
            outcome.bores
        );
    }

    // ----- Refusal flow-through -----

    /// A bore whose wall's outer loop carries a SKEWED (non-axis-
    /// perpendicular) rim must read `Unverifiable` naming the topology
    /// defect -- never a fabricated depth/thru-blind bit.
    #[test]
    fn unreadable_rim_bore_is_unverifiable_not_guessed() {
        let (model, solid_id, bore_face) = plate_with_unreadable_bore(20.0, 20.0, 10.0, 3.0);
        let outcome = bore_metrics(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.bores.is_empty(),
            "an unreadable bore must never be fabricated into an exact record: {:?}",
            outcome.bores
        );
        assert_eq!(outcome.unverifiable.len(), 1);
        assert_eq!(outcome.unverifiable[0].face, bore_face);
        assert!(matches!(
            outcome.unverifiable[0].reason,
            UnverifiableReason::UnsupportedTopology { .. }
        ));
    }

    /// Malformed input: a dangling `solid_id` is an `Err`, never a
    /// refusal value (spec §4).
    #[test]
    fn dangling_solid_ref_is_an_error_not_a_refusal() {
        let model = BRepModel::new();
        let result = bore_metrics(&model, 999);
        assert!(matches!(
            result,
            Err(DfmError::DanglingSolidRef { solid: 999 })
        ));
    }
}
