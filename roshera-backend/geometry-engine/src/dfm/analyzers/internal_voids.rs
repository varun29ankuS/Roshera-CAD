//! `internal_voids` — fully-enclosed cavities proven from topology (spec
//! §3.1 / S5).
//!
//! ## Candidates: a solid's inner shells (task scope)
//!
//! `Solid.inner_shells` are the candidate voids — `bore.rs`'s own
//! `solid_faces` helper already reads this field (outer shell + every
//! inner shell) the same way, reused verbatim here via
//! [`crate::dfm::analyzers::bore::solid_faces`]'s SIBLING scope: this
//! module only needs the SHELL ids themselves, not their faces flattened
//! together, so it reads `solid.inner_shells` directly rather than through
//! that helper. Spec §3.1 also lists "solids enclosed by others in the
//! same model" as a second candidate source (one whole `Solid` nested
//! inside another's volume) — NOT attempted here (named v1 non-goal,
//! absence ≠ oversight, same convention `bore.rs` uses for non-cylindrical
//! holes): it needs a cross-solid containment test this slice has no
//! machinery for, and the executor brief scopes this analyzer to
//! `Solid.inner_shells` explicitly.
//!
//! ## Enclosure must be PROVEN, not read off a label
//!
//! [`crate::primitives::shell::ShellType::Closed`] is a caller-supplied
//! LABEL (`Shell::new(id, shell_type)` — any code can construct a shell and
//! declare it `Closed`); trusting it is exactly the "assumed, not derived"
//! failure mode `analyzers/orientation.rs`'s module docs warn about for a
//! face's stale `uv_bounds`/`angle_limits` after a boolean trim. This
//! analyzer does NOT read `shell_type` at all. Instead it derives
//! watertightness the same way [`crate::primitives::shell::Shell::build_connectivity`]
//! classifies edges (`shell.rs`: 1 face-use ⇒ boundary, 2 ⇒ manifold, ≥3 ⇒
//! non-manifold) — but read-only, over THIS shell's own faces/loops only,
//! rather than calling `build_connectivity` itself: that method takes
//! `&mut Shell` and populates a persistent cache, which would make this
//! analyzer a hidden mutator (every other `dfm` analyzer is read-only —
//! `edge_classification::classify_edge`'s own doc states the same
//! discipline: "Read-only — does not mutate the model"), and this
//! analyzer's own need (a single boolean verdict) is a strict subset of
//! what `build_connectivity` computes and caches. A shell is proven
//! watertight iff it is non-empty AND every boundary edge (across its own
//! faces' outer + every inner loop) is used by EXACTLY two of those faces —
//! the standard closed-2-manifold criterion. Any other count (0 relevant
//! here since we only count edges that ARE used; 1 = a real boundary gap,
//! ≥3 = non-manifold) means enclosure CANNOT be proven, and the shell
//! refuses (`UnverifiableReason::UnsoundPrecondition`, the exact refusal
//! `report.rs`'s own doc anticipates for this analyzer) rather than being
//! silently trusted.
//!
//! **Named v1 limitation:** this proves LOCAL watertightness (every edge
//! used twice), not global connectedness — a shell built from two disjoint
//! closed components would still pass. Out of scope for this slice; a
//! bare-store fixture cannot construct that pathology by accident, and no
//! producer in this tree emits disconnected multi-component shells.
//!
//! ## Volume: exact-or-none, never fabricated
//!
//! Per the executor brief, a proven void is reported WITHOUT a volume
//! rather than an approximated one when exact computation is not possible.
//! This module computes volume exactly ONLY when every face of the shell is
//! a `Plane` whose outer loop classifies as an exact rectangle (reusing
//! [`crate::dfm::analyzers::thickness::rectangle_from_outer_loop`] verbatim
//! — the no-copies discipline, same promotion precedent as `axial_extent`'s
//! S4 reuse) — an ALL-OR-NOTHING test: a single non-planar or
//! non-rectangular face anywhere in the shell drops the WHOLE shell's
//! volume to `None`, never a partial sum. The per-face contribution is the
//! standard divergence-theorem term
//! `(outward_normal · centroid) * area / 3`, summed over every face — the
//! SAME formula [`crate::primitives::shell::Shell::compute_mass_properties`]
//! uses (`shell.rs`, "Volume by Gauss / divergence theorem"), specialised
//! here to the exact rectangle case (closed form centroid/area, no
//! numerical face-stats dependency). The outward normal is
//! `face.orientation.sign() * plane.normal` — NOT `u_dir × v_dir`, whose
//! chirality depends on which edge the loop-walk in
//! `rectangle_from_outer_loop` happened to start from and is not a
//! reliable outward-normal source. The raw signed sum's ABSOLUTE VALUE is
//! reported (mirrors `Shell::compute_mass_properties`'s own
//! `Some(volume.abs())`, `shell.rs`: the sign is an artefact of loop
//! traversal direction, not a physically meaningful quantity for a single
//! independent void).

use crate::dfm::analyzers::thickness::rectangle_from_outer_loop;
use crate::dfm::report::{Derivation, DfmError, DfmValue, SurfaceKind, UnverifiableReason};
use crate::primitives::shell::{Shell, ShellId};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Plane;
use crate::primitives::topology_builder::BRepModel;
use std::collections::HashMap;

/// One proven internal void (spec §3.1): a `Solid.inner_shells` entry whose
/// watertightness is proven from topology (module docs), not assumed.
/// `volume` is `None` when exact computation is not possible for this
/// shell's face mix — a void reported without a volume, never a fabricated
/// one.
#[derive(Debug, Clone, PartialEq)]
pub struct VoidRecord {
    pub shell: ShellId,
    pub volume: Option<DfmValue>,
}

/// An inner shell whose enclosure could not be proven from topology (module
/// docs) — not itself a Pass/Violation decision.
#[derive(Debug, Clone, PartialEq)]
pub struct UnverifiableVoid {
    pub shell: ShellId,
    pub reason: UnverifiableReason,
}

/// The full result of one `internal_voids` call (spec §4: a refusal is a
/// VALUE, never an error — [`DfmError`] is reserved for malformed input: a
/// dangling `solid_id`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InternalVoidsOutcome {
    pub voids: Vec<VoidRecord>,
    pub unverifiable: Vec<UnverifiableVoid>,
}

/// Module docs' derived watertightness proof: `shell` is watertight iff it
/// is non-empty and every edge referenced by its own faces' outer + inner
/// loops is referenced by EXACTLY two of those faces. Read-only — does not
/// touch `Shell::build_connectivity`'s cache (module docs explain why).
fn shell_is_watertight(model: &BRepModel, shell: &Shell) -> bool {
    if shell.faces.is_empty() {
        return false; // an empty shell proves nothing
    }
    let mut use_count: HashMap<u32, u32> = HashMap::new();
    for &face_id in &shell.faces {
        let Some(face) = model.faces.get(face_id) else {
            return false; // dangling face reference: cannot prove closure
        };
        let mut loops = vec![face.outer_loop];
        loops.extend_from_slice(&face.inner_loops);
        for lid in loops {
            let Some(lp) = model.loops.get(lid) else {
                continue; // an empty/placeholder loop contributes nothing
            };
            for &eid in &lp.edges {
                *use_count.entry(eid).or_insert(0) += 1;
            }
        }
    }
    !use_count.is_empty() && use_count.values().all(|&c| c == 2)
}

/// Module docs' exact-or-none volume: `Some` only when every face of
/// `shell` is a `Plane` whose outer loop is an exact rectangle
/// (all-or-nothing — a single disqualifying face returns `None` for the
/// WHOLE shell, never a partial sum).
fn exact_planar_shell_volume(model: &BRepModel, shell: &Shell) -> Option<DfmValue> {
    if shell.faces.is_empty() {
        return None;
    }
    let mut total = 0.0_f64;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id)?;
        let surface = model.surfaces.get(face.surface_id)?;
        let plane = surface.as_any().downcast_ref::<Plane>()?;
        let rect = rectangle_from_outer_loop(face, &model.loops, &model.edges, &model.curves)?;
        let normal = plane.normal * face.orientation.sign();
        let area = rect.s_len * rect.t_len;
        let centroid =
            rect.origin + rect.u_dir * (rect.s_len / 2.0) + rect.v_dir * (rect.t_len / 2.0);
        total += centroid.to_vec().dot(&normal) * area / 3.0;
    }
    Some(DfmValue::new(
        total.abs(),
        Derivation::Analytic {
            surface_type: SurfaceKind::Plane,
            method: "exact enclosed volume: divergence theorem over planar rectangular \
                     faces, sum of (outward_normal . centroid) * area / 3"
                .to_string(),
        },
    ))
}

/// `internal_voids` (spec §3.1, S5): every proven internal void of
/// `solid_id`'s `inner_shells`, or an honest per-shell refusal (module
/// docs). Returns `Err` only for a dangling `solid_id` (spec §4).
pub fn internal_voids(
    model: &BRepModel,
    solid_id: SolidId,
) -> Result<InternalVoidsOutcome, DfmError> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or(DfmError::DanglingSolidRef { solid: solid_id })?;

    let mut voids = Vec::new();
    let mut unverifiable = Vec::new();

    for &shell_id in &solid.inner_shells {
        let Some(shell) = model.shells.get(shell_id) else {
            unverifiable.push(UnverifiableVoid {
                shell: shell_id,
                reason: UnverifiableReason::UnsoundPrecondition {
                    detail: format!(
                        "inner shell {shell_id} does not resolve in the model; enclosure \
                         cannot be proven"
                    ),
                },
            });
            continue;
        };

        if !shell_is_watertight(model, shell) {
            unverifiable.push(UnverifiableVoid {
                shell: shell_id,
                reason: UnverifiableReason::UnsoundPrecondition {
                    detail: format!(
                        "inner shell {shell_id}: not every boundary edge is shared by exactly \
                         two faces within the shell; enclosure cannot be proven from topology \
                         (a `ShellType` label is not trusted -- module docs)"
                    ),
                },
            });
            continue;
        }

        let volume = exact_planar_shell_volume(model, shell);
        voids.push(VoidRecord {
            shell: shell_id,
            volume,
        });
    }

    voids.sort_by_key(|v| v.shell);
    unverifiable.sort_by_key(|u| u.shell);

    Ok(InternalVoidsOutcome {
        voids,
        unverifiable,
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Bare-store solid fixtures (no boolean/extrude -- the same
    //! `KNOWN_REDS`-avoidance convention every other `dfm` analyzer's test
    //! module documents). `pub(crate)` so `packs::fdm`'s
    //! `fdm.trapped_volume` tests can reuse
    //! [`solid_with_enclosed_box_void`] instead of re-deriving the same
    //! hand-built enclosed-cavity geometry a second time (the no-copies
    //! discipline, same convention `bore.rs`'s own `fixtures` module
    //! establishes for `fdm.min_bore`'s tests).

    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::{CurveStore, Line, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation, EdgeStore};
    use crate::primitives::face::{Face, FaceId, FaceOrientation, FaceStore};
    use crate::primitives::r#loop::{Loop, LoopStore, LoopType};
    use crate::primitives::shell::ShellType;
    use crate::primitives::solid::Solid;
    use crate::primitives::surface::SurfaceStore;

    pub(crate) struct Stores {
        surfaces: SurfaceStore,
        faces: FaceStore,
        loops: LoopStore,
        edges: EdgeStore,
        curves: CurveStore,
    }

    impl Stores {
        pub(crate) fn new() -> Self {
            Self {
                surfaces: SurfaceStore::new(),
                faces: FaceStore::new(),
                loops: LoopStore::new(),
                edges: EdgeStore::new(),
                curves: CurveStore::new(),
            }
        }

        /// A straight edge from `a` to `b` -- a fresh `Line` curve wrapped
        /// in an `Edge`, returning its id so callers can reference the SAME
        /// edge from more than one face's loop (a real B-Rep shared edge,
        /// not two coincident-but-distinct ones).
        pub(crate) fn add_edge(&mut self, a: Point3, b: Point3) -> crate::primitives::edge::EdgeId {
            let curve_id = self.curves.add(Box::new(Line::new(a, b)));
            self.edges.add(Edge::new(
                0,
                0,
                1,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            ))
        }

        /// A rectangular PLANE face with outward normal `normal`, built
        /// from FOUR ALREADY-CREATED edge ids (in cycle order) rather than
        /// fresh `Line`s -- lets a caller share edges between adjacent
        /// faces (module docs' watertightness proof needs a genuine
        /// 2-face-per-edge B-Rep, not 6 independently-edged rectangles that
        /// merely happen to be coincident).
        pub(crate) fn add_rectangle_face_from_edges(
            &mut self,
            normal: Vector3,
            plane_point: Point3,
            edge_ids: [crate::primitives::edge::EdgeId; 4],
        ) -> FaceId {
            let plane = Plane::from_point_normal(plane_point, normal)
                .unwrap_or_else(|e| panic!("plane: {e}"));
            let surface_id = self.surfaces.add(Box::new(plane));

            let mut loop_ = Loop::new(0, LoopType::Outer);
            for edge_id in edge_ids {
                loop_.add_edge(edge_id, true);
            }
            let outer_loop = self.loops.add(loop_);
            let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
            self.faces.add(face)
        }
    }

    /// A closed 6-face box `[x0,x1] x [y0,y1] x [z0,z1]`, each face's
    /// outward normal set directly to the box's own outward direction, and
    /// EVERY one of its 12 edges genuinely SHARED between exactly the two
    /// faces that meet there (module docs' derived watertightness proof
    /// needs this -- 6 independently-edged rectangles that are merely
    /// coincident, `thickness.rs`'s own box fixture's convention, would
    /// read as 24 boundary edges, not a closed shell).
    pub(crate) fn add_box_shell(
        s: &mut Stores,
        x0: f64,
        x1: f64,
        y0: f64,
        y1: f64,
        z0: f64,
        z1: f64,
    ) -> Vec<FaceId> {
        let c0 = Point3::new(x0, y0, z0);
        let c1 = Point3::new(x1, y0, z0);
        let c2 = Point3::new(x1, y1, z0);
        let c3 = Point3::new(x0, y1, z0);
        let c4 = Point3::new(x0, y0, z1);
        let c5 = Point3::new(x1, y0, z1);
        let c6 = Point3::new(x1, y1, z1);
        let c7 = Point3::new(x0, y1, z1);

        let e01 = s.add_edge(c0, c1);
        let e12 = s.add_edge(c1, c2);
        let e23 = s.add_edge(c2, c3);
        let e30 = s.add_edge(c3, c0);
        let e45 = s.add_edge(c4, c5);
        let e56 = s.add_edge(c5, c6);
        let e67 = s.add_edge(c6, c7);
        let e74 = s.add_edge(c7, c4);
        let e04 = s.add_edge(c0, c4);
        let e15 = s.add_edge(c1, c5);
        let e26 = s.add_edge(c2, c6);
        let e37 = s.add_edge(c3, c7);

        vec![
            // bottom, z0, normal -Z
            s.add_rectangle_face_from_edges(Vector3::new(0.0, 0.0, -1.0), c0, [e01, e12, e23, e30]),
            // top, z1, normal +Z
            s.add_rectangle_face_from_edges(Vector3::new(0.0, 0.0, 1.0), c4, [e45, e56, e67, e74]),
            // front, y0, normal -Y
            s.add_rectangle_face_from_edges(Vector3::new(0.0, -1.0, 0.0), c0, [e01, e15, e45, e04]),
            // back, y1, normal +Y
            s.add_rectangle_face_from_edges(Vector3::new(0.0, 1.0, 0.0), c3, [e23, e26, e67, e37]),
            // left, x0, normal -X
            s.add_rectangle_face_from_edges(Vector3::new(-1.0, 0.0, 0.0), c0, [e30, e04, e74, e37]),
            // right, x1, normal +X
            s.add_rectangle_face_from_edges(Vector3::new(1.0, 0.0, 0.0), c1, [e12, e26, e56, e15]),
        ]
    }

    pub(crate) fn wrap_shell(
        model: &mut BRepModel,
        faces: &[FaceId],
    ) -> crate::primitives::shell::ShellId {
        let mut shell = crate::primitives::shell::Shell::new(0, ShellType::Closed);
        shell.add_faces(faces);
        model.shells.add(shell)
    }

    pub(crate) fn empty_model_from(s: Stores) -> BRepModel {
        let mut model = BRepModel::new();
        model.surfaces = s.surfaces;
        model.faces = s.faces;
        model.loops = s.loops;
        model.edges = s.edges;
        model.curves = s.curves;
        model
    }

    /// An outer 10x10x10 box shell with a genuinely enclosed, disjoint
    /// inner box void `[2,4] x [2,5] x [2,6]` (dims 2x3x4 -> volume 24) --
    /// every face planar-rectangular, every edge shared by exactly two
    /// faces within its own shell. The headline hand-computed exact-volume
    /// fixture, reused verbatim by `packs::fdm`'s `fdm.trapped_volume`
    /// tests.
    pub(crate) fn solid_with_enclosed_box_void() -> (BRepModel, crate::primitives::solid::SolidId) {
        let mut s = Stores::new();
        let outer_faces = add_box_shell(&mut s, 0.0, 10.0, 0.0, 10.0, 0.0, 10.0);
        let inner_faces = add_box_shell(&mut s, 2.0, 4.0, 2.0, 5.0, 2.0, 6.0);
        let mut model = empty_model_from(s);

        let outer_shell = wrap_shell(&mut model, &outer_faces);
        let inner_shell = wrap_shell(&mut model, &inner_faces);
        let mut solid = Solid::new(0, outer_shell);
        solid.add_inner_shell(inner_shell);
        let solid_id = model.solids.add(solid);
        (model, solid_id)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{
        add_box_shell, empty_model_from, solid_with_enclosed_box_void, wrap_shell, Stores,
    };
    use super::*;
    use crate::primitives::solid::Solid;

    // ----- Hand-computed exact case: a proven enclosed cavity -----

    /// See [`fixtures::solid_with_enclosed_box_void`] for the geometry.
    /// Exact volume asserted: 24.
    #[test]
    fn enclosed_box_cavity_reports_exact_volume() {
        let (model, solid_id) = solid_with_enclosed_box_void();
        let solid = model
            .solids
            .get(solid_id)
            .unwrap_or_else(|| panic!("fixture solid resolves"));
        let inner_shell = solid.inner_shells[0];

        let outcome = internal_voids(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.unverifiable.is_empty(),
            "a genuinely watertight inner shell must not refuse: {:?}",
            outcome.unverifiable
        );
        assert_eq!(
            outcome.voids.len(),
            1,
            "expected exactly one void: {:?}",
            outcome.voids
        );
        assert_eq!(outcome.voids[0].shell, inner_shell);
        let volume = outcome.voids[0]
            .volume
            .as_ref()
            .unwrap_or_else(|| panic!("expected an exact volume, got None"));
        assert!(
            (volume.value - 24.0).abs() < 1e-9,
            "volume = {}",
            volume.value
        );
    }

    // ----- THE ANTI-FABRICATION HEADLINE -----

    /// A through-hole (S4's own headline fixture) has NO inner shells at
    /// all -- the bore wall is part of the single connected outer shell,
    /// never a separate closed shell (a through-hole is genus on the
    /// outer shell, not an enclosed cavity). `internal_voids` must report
    /// nothing at all for it: no voids, no refusals.
    #[test]
    fn through_hole_is_never_reported_as_an_internal_void() {
        use crate::dfm::analyzers::bore::fixtures::plate_with_through_bore;
        let (model, solid_id, _bore_face) = plate_with_through_bore(20.0, 20.0, 10.0, 3.0);

        let outcome = internal_voids(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.voids.is_empty(),
            "a through-hole must never be fabricated into a void: {:?}",
            outcome.voids
        );
        assert!(
            outcome.unverifiable.is_empty(),
            "a through-hole is not even a refusal candidate -- there is no inner shell at \
             all: {:?}",
            outcome.unverifiable
        );
    }

    // ----- Refusal: enclosure cannot be proven -----

    /// A genuinely NON-watertight "inner shell" -- one face of the 6-face
    /// box is simply omitted, leaving 4 boundary edges used by only one
    /// face within the shell -- must REFUSE, never be assumed enclosed
    /// just because it sits in `Solid.inner_shells` and is stored as
    /// `ShellType::Closed`.
    #[test]
    fn non_watertight_inner_shell_refuses_enclosure_is_not_assumed() {
        let mut s = Stores::new();
        let outer_faces = add_box_shell(&mut s, 0.0, 10.0, 0.0, 10.0, 0.0, 10.0);
        let mut inner_faces = add_box_shell(&mut s, 2.0, 4.0, 2.0, 5.0, 2.0, 6.0);
        inner_faces.pop(); // drop the top face -- the shell no longer closes
        let mut model = empty_model_from(s);

        let outer_shell = wrap_shell(&mut model, &outer_faces);
        // Declared Closed anyway -- the label must not be trusted (module
        // docs): this is the exact case the derived boundary-edge count
        // exists to catch.
        let inner_shell = wrap_shell(&mut model, &inner_faces);
        let mut solid = Solid::new(0, outer_shell);
        solid.add_inner_shell(inner_shell);
        let solid_id = model.solids.add(solid);

        let outcome = internal_voids(&model, solid_id)
            .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.voids.is_empty(),
            "a non-watertight shell must never be reported as a proven void: {:?}",
            outcome.voids
        );
        assert_eq!(outcome.unverifiable.len(), 1);
        assert_eq!(outcome.unverifiable[0].shell, inner_shell);
        assert!(matches!(
            outcome.unverifiable[0].reason,
            UnverifiableReason::UnsoundPrecondition { .. }
        ));
    }

    /// Malformed input: a dangling `solid_id` is an `Err`, never a
    /// refusal value (spec §4).
    #[test]
    fn dangling_solid_ref_is_an_error_not_a_refusal() {
        let model = BRepModel::new();
        let result = internal_voids(&model, 999);
        assert!(matches!(
            result,
            Err(DfmError::DanglingSolidRef { solid: 999 })
        ));
    }
}
