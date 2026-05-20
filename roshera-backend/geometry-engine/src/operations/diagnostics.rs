//! Structured taxonomy for blend (fillet / chamfer / sew) failure modes.
//!
//! # Why a separate enum instead of free-form strings?
//!
//! Until this slice, every blend-side failure collapsed into one of
//! [`OperationError::InvalidGeometry`], [`OperationError::InvalidInput`],
//! or [`OperationError::NumericalError`] carrying a human-readable
//! `String` payload. That payload is fine for a human reading a log
//! line but useless for an agent (Claude, an MCP tool, or the
//! frontend's diagnostic surface) trying to decide *what to do next*:
//! "should I try a smaller radius? a different selection? a different
//! solver?". The string format also has no stability guarantee — the
//! kernel was free to reword it on any commit and silently break
//! every downstream regex.
//!
//! [`BlendFailure`] replaces the unstructured string with a tagged
//! enum carrying the *minimum* set of fields a caller needs to:
//!
//! - **Branch** on the failure category (`match` on the variant).
//! - **Recover** automatically where possible (e.g. on
//!   `RadiusExceedsCurvature` retry with `r_max * 0.95`).
//! - **Localise** the offending entity for highlight / pan-to in the
//!   viewport (every variant carries the `EdgeId` or `VertexId` that
//!   caused the failure).
//!
//! # This slice (Phase-1) is additive
//!
//! The full Diagnostics-α plan (Task #12) ends with [`OperationError`]
//! gaining a dedicated `BlendFailed(Box<BlendFailure>)` variant that
//! serialises through the REST surface as JSON. Phase-1 lands the
//! taxonomy and the [`From<BlendFailure>`] conversion only — every
//! existing call site keeps emitting the same legacy [`OperationError`]
//! variants, so this commit is behaviour-preserving. Subsequent
//! slices swap failure sites to `Err(blend_failure.into())` and the
//! payload upgrades follow naturally.
//!
//! # Variants
//!
//! Each variant maps onto a concrete failure site in the blend
//! pipeline:
//!
//! | Variant                       | Site                                                          |
//! |-------------------------------|---------------------------------------------------------------|
//! | [`RadiusExceedsCurvature`]    | `lifecycle::validate_can_apply` (F6-α, Task #9)               |
//! | [`SetbackTooLong`]            | `lifecycle::validate_corner_compatibility` (F2-γ.1)           |
//! | [`DihedralInflection`]        | `fillet::detect_dihedral_inflection` (fillet.rs, Task #98)    |
//! | [`SewGapTooLarge`]            | `sew::sew_faces` (F7-δ)                                       |
//! | [`SpineSolverDiverged`]       | `spine_solver::solve_*` (F3-γ marching)                       |
//! | [`VertexBlendUnsupported`]    | corner patch dispatch (F5-α / F5-β)                           |
//! | [`TopologyViolation`]         | catch-all — replace with a specific variant when discovered   |
//!
//! [`RadiusExceedsCurvature`]: BlendFailure::RadiusExceedsCurvature
//! [`SetbackTooLong`]: BlendFailure::SetbackTooLong
//! [`DihedralInflection`]: BlendFailure::DihedralInflection
//! [`SewGapTooLarge`]: BlendFailure::SewGapTooLarge
//! [`SpineSolverDiverged`]: BlendFailure::SpineSolverDiverged
//! [`VertexBlendUnsupported`]: BlendFailure::VertexBlendUnsupported
//! [`TopologyViolation`]: BlendFailure::TopologyViolation

use super::blend_graph::BlendVertexKind;
use super::OperationError;
use crate::primitives::edge::EdgeId;
use crate::primitives::vertex::VertexId;

// CF-β re-export so the mixed-kind reason variant + wire shape both
// reach for the same set type the storage layer uses.
pub use crate::primitives::solid::VertexBlendKindSet;

/// CF-β — specific reason a mixed-kind corner (a vertex carrying both
/// a fillet and a chamfer on different incident edges) cannot be
/// stitched at this slice. Nested inside
/// [`VertexBlendUnsupportedReason::MixedKindUnsupported`].
///
/// Each variant is a hard-reject sub-case: the kernel surfaces the
/// most specific reason it can determine pre-flight so agents can
/// branch on the failure without parsing strings. CF-β.3 implements
/// the only currently-feasible case (degree-3 convex equal-displacement
/// box corner); β.2 ships the typed-reject surface so the wire shape
/// is pinned before any geometry lands.
///
/// Internally tagged on `type` so a deserialiser can branch on the
/// variant without knowing the surrounding `BlendFailure` shape:
///
/// ```json
/// { "type": "MixedDisplacements", "offsets": [0.5], "radii": [0.8] }
/// ```
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum MixedKindRejectDetail {
    /// Corner degree (number of incident blend edges) exceeds the
    /// 3-edge convex case the mixed-kind cap synthesizer supports.
    /// CF-β.3 only implements degree 3; higher-degree mixed corners
    /// await follow-up work.
    DegreeUnsupported {
        /// Incident-blend-edge degree at the vertex.
        degree: usize,
    },
    /// Chamfer offset(s) and fillet radius/radii at the same corner
    /// disagree beyond numerical tolerance. The mixed cap loses
    /// coplanarity the moment displacements differ — see CF-β plan
    /// §3.3 for the proof.
    MixedDisplacements {
        /// Recorded chamfer offsets at the corner's chamfer edges.
        offsets: Vec<f64>,
        /// Recorded fillet radii at the corner's fillet edges.
        radii: Vec<f64>,
    },
    /// The mixed cap's loop endpoints failed the planarity gate.
    /// Adjacent face curvature or numerical drift pushed the residual
    /// above tolerance.
    NonPlanarCap {
        /// Worst-case point-to-plane residual.
        residual: f64,
        /// Tolerance the residual was compared against.
        tolerance: f64,
    },
    /// At least one face adjacent to the corner is curved. CF-β only
    /// supports planar-adjacent corners (the chamfer cap's existing
    /// `cap_vertices_coplanar` precondition).
    CurvedAdjacent,
    /// Corner classification reached this gate with a non-convex sign
    /// (concave or Cliff). The upstream F2-γ.1 corner-compatibility
    /// check normally rejects these earlier; the variant exists so
    /// the mixed gate can still emit a typed signal if the upstream
    /// pass is bypassed (tests).
    ConcaveOrCliff,
}

impl std::fmt::Display for MixedKindRejectDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MixedKindRejectDetail::DegreeUnsupported { degree } => write!(
                f,
                "mixed-kind corner degree {} exceeds the 3-edge case CF-β supports",
                degree
            ),
            MixedKindRejectDetail::MixedDisplacements { offsets, radii } => write!(
                f,
                "mixed-kind corner displacements disagree (chamfer offsets {:?}, fillet radii {:?})",
                offsets, radii
            ),
            MixedKindRejectDetail::NonPlanarCap {
                residual,
                tolerance,
            } => write!(
                f,
                "mixed-kind cap planarity residual {} exceeds tolerance {}",
                residual, tolerance
            ),
            MixedKindRejectDetail::CurvedAdjacent => write!(
                f,
                "mixed-kind corner has a curved adjacent face — planar cap inadmissible"
            ),
            MixedKindRejectDetail::ConcaveOrCliff => write!(
                f,
                "mixed-kind corner is concave or Cliff — single-sign cap not defined"
            ),
        }
    }
}

/// Why a corner-patch (vertex blend) cannot be constructed at the
/// given vertex. Carried as a field of
/// [`BlendFailure::VertexBlendUnsupported`] so agents can branch on
/// the *underlying topological reason* — degree vs. mixed convexity
/// vs. non-manifold neighbourhood — rather than parsing strings.
// Note: cannot derive `Eq` because the CF-β `MixedKindUnsupported`
// variant nests `MixedKindRejectDetail`, which carries `f64` fields
// (planarity residual / mixed displacements). All sibling variants
// remain comparable through `PartialEq`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum VertexBlendUnsupportedReason {
    /// Vertex incidence exceeds the highest-supported corner patch.
    /// F5-α (Task #10) implements the three-edge convex equal-radius
    /// ball corner; higher degrees await F5-β.
    DegreeTooHigh {
        /// Number of incident blend edges at the vertex.
        degree: usize,
    },
    /// At least one incident edge has no defined dihedral
    /// ([`BlendVertexKind::Cliff`]) — non-manifold or seam topology
    /// reached the corner. Cannot proceed without first resolving
    /// the upstream topology.
    NonManifoldNeighbourhood,
    /// Incident edges mix convex and concave dihedrals
    /// ([`BlendVertexKind::Mixed`]). Single-sign corner patches are
    /// not defined for sign-change neighbourhoods.
    MixedConvexity,
    /// Vertex is [`BlendVertexKind::Smooth`] — no corner exists to
    /// blend, the dihedral is continuous through the vertex.
    SmoothVertex,
    /// Incident blend edges carry per-edge radii that disagree beyond
    /// numerical tolerance. The F5-α equal-radius apex-sphere corner
    /// assumes one ball radius across all incident edges; mixed-
    /// radius corners need an n-rail / Gregory patch (F5-β).
    MixedRadii,
    /// At least one face adjacent to the corner is curved, so the
    /// N cap-edge endpoints do not lie within `tolerance` of a
    /// single plane. The Chamfer-β planar n-gon cap requires
    /// coplanar corner vertices; curved-adjacent corners need a
    /// curved cap (Chamfer-γ, future).
    CurvedAdjacent,
    /// CF-β — the corner carries both a fillet and a chamfer on
    /// different incident edges, but the mixed-kind cap synthesizer
    /// cannot stitch this specific configuration. The nested
    /// [`MixedKindRejectDetail`] carries the most specific reason
    /// the pre-flight could determine (degree, displacement mismatch,
    /// planarity, curved adjacency, or concavity).
    ///
    /// `existing` is the set of kinds already recorded at the vertex
    /// before the current call; `requested` is the kind the current
    /// call would have added. Agents can branch on `existing.is_mixed()`
    /// to distinguish "third call into an already-mixed corner" from
    /// "second call upgrading a single-kind corner to mixed".
    MixedKindUnsupported {
        /// Set of kinds already recorded at the vertex.
        existing: VertexBlendKindSet,
        /// Kind the current call would have added.
        requested: crate::primitives::solid::BlendKind,
        /// Specific obstruction within the mixed-kind cap synthesizer.
        detail: MixedKindRejectDetail,
    },
}

impl std::fmt::Display for VertexBlendUnsupportedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VertexBlendUnsupportedReason::DegreeTooHigh { degree } => write!(
                f,
                "vertex degree {} exceeds the highest-supported corner patch",
                degree
            ),
            VertexBlendUnsupportedReason::NonManifoldNeighbourhood => {
                write!(f, "non-manifold or seam edge incident at the vertex")
            }
            VertexBlendUnsupportedReason::MixedConvexity => {
                write!(f, "mixed convex/concave incidence — sign change not supported")
            }
            VertexBlendUnsupportedReason::SmoothVertex => {
                write!(f, "vertex is smooth — no corner to blend")
            }
            VertexBlendUnsupportedReason::MixedRadii => {
                write!(f, "incident blend radii disagree — apex sphere undefined")
            }
            VertexBlendUnsupportedReason::CurvedAdjacent => {
                write!(
                    f,
                    "adjacent face is curved — cap corner vertices not coplanar"
                )
            }
            VertexBlendUnsupportedReason::MixedKindUnsupported {
                existing,
                requested,
                detail,
            } => write!(
                f,
                "mixed-kind corner (existing {}, requested {}): {}",
                existing, requested, detail
            ),
        }
    }
}

/// Structured taxonomy of blend (fillet / chamfer / sew) failures.
///
/// See the [module docs](self) for the rationale and the call-site
/// mapping table.
///
/// # JSON shape
///
/// `BlendFailure` is internally tagged on `type`. A
/// `RadiusExceedsCurvature` round-trips as:
///
/// ```json
/// {
///   "type": "RadiusExceedsCurvature",
///   "edge": 7,
///   "station": 0.42,
///   "r_requested": 2.0,
///   "r_max": 1.25
/// }
/// ```
///
/// This is the surface the api-server's REST and agent endpoints
/// expose. The tag layout is part of the contract — changing it is
/// a breaking change to the agent surface.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum BlendFailure {
    /// The requested blend radius exceeds the local feature curvature
    /// at the given station. Recoverable by retrying with `r ≤ r_max`.
    RadiusExceedsCurvature {
        /// Edge whose curvature is too tight.
        edge: EdgeId,
        /// Arc-length parameter ∈ [0, 1] where the test failed.
        station: f64,
        /// Radius the caller asked for.
        r_requested: f64,
        /// Largest feasible radius at this station (1 / |κ_max|).
        r_max: f64,
    },
    /// Corner setback extends beyond the edge length. Adjacent blends
    /// would overlap; F2-γ.1 rejects this at validate-time.
    SetbackTooLong {
        /// Vertex whose setback overruns.
        vertex: VertexId,
        /// Setback distance the corner patch needs.
        setback: f64,
        /// Length of the shortest incident edge.
        edge_length: f64,
    },
    /// Dihedral angle along the edge passes through 0 / π — convexity
    /// flips. Single-radius rolling-ball blends are undefined across
    /// the inflection.
    DihedralInflection {
        /// Edge whose dihedral inverts along its length.
        edge: EdgeId,
        /// Parameter ∈ [0, 1] of the inflection point.
        station: f64,
        /// Dihedral angle at the inflection in degrees, ∈ (-180, 180).
        dihedral_deg: f64,
    },
    /// Sew step (F7-δ) found a gap between the blend surface and a
    /// trimmed neighbour exceeding `tolerance`. Usually means the
    /// rolling-ball walk and the trim curve disagree on the seam path.
    SewGapTooLarge {
        /// Edge being sewn.
        edge: EdgeId,
        /// Measured gap.
        gap: f64,
        /// Tolerance the gap was compared against.
        tolerance: f64,
    },
    /// Spine solver (F3-γ marching) failed to converge at the given
    /// station. `residual` is the final residual the solver gave up
    /// at — useful for tuning solver iteration budgets.
    SpineSolverDiverged {
        /// Edge whose spine is being solved.
        edge: EdgeId,
        /// Parameter ∈ [0, 1] where divergence occurred.
        station: f64,
        /// Final residual.
        residual: f64,
    },
    /// Vertex (corner) blend cannot be constructed. The `kind` is the
    /// vertex's classification per [`BlendVertexKind`]; `reason` is
    /// the specific obstruction.
    VertexBlendUnsupported {
        /// Vertex whose corner cannot be patched.
        vertex: VertexId,
        /// Topology classification at the vertex.
        kind: BlendVertexKind,
        /// Specific reason within `kind`.
        reason: VertexBlendUnsupportedReason,
    },
    /// CF-α — the requested blend kind conflicts with a previously
    /// applied blend on the same edge (or on a vertex shared with a
    /// previously-blended edge). Surfaced pre-flight so the caller
    /// gets a typed, actionable signal instead of the legacy
    /// `edge not found in model` shape that results from the
    /// original edge having been destroyed by `splice_blend_edge`.
    ///
    /// `existing_kind == requested_kind` is also a conflict — the
    /// edge no longer exists in the model topology even for a
    /// same-kind retry — but carries a different remediation hint
    /// (the requested edge was already processed) than the
    /// cross-kind case (fillet ↔ chamfer mix at a shared corner is
    /// not supported by the kernel at this slice).
    ConflictingBlendKind {
        /// The edge the caller requested (by ID). Either this edge
        /// has been removed by a previous blend, or it is incident
        /// to a vertex that survives from a previous blend of the
        /// opposite kind.
        edge: EdgeId,
        /// Kind already applied (recorded on the host `Solid`).
        existing_kind: BlendKind,
        /// Kind requested by the current call site.
        requested_kind: BlendKind,
    },
    /// Catch-all for irreducible failures not yet classified. The
    /// `detail` string is freeform; treat occurrences as a TODO to
    /// replace with a structured variant once the failure mode is
    /// understood.
    TopologyViolation {
        /// Human-readable detail.
        detail: String,
    },
}

// `BlendKind` is defined alongside the per-`Solid` blend registry in
// `primitives::solid` (the kernel layer that physically stores it),
// then re-exported here so callers using the diagnostics surface
// have a single import site for `BlendFailure::ConflictingBlendKind`
// and the kind tag it carries.
pub use crate::primitives::solid::BlendKind;

impl std::fmt::Display for BlendFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlendFailure::RadiusExceedsCurvature {
                edge,
                station,
                r_requested,
                r_max,
            } => write!(
                f,
                "blend radius {} at edge {} station {:.3} exceeds local curvature limit r_max={}",
                r_requested, edge, station, r_max
            ),
            BlendFailure::SetbackTooLong {
                vertex,
                setback,
                edge_length,
            } => write!(
                f,
                "corner setback {} at vertex {} exceeds shortest incident edge length {}",
                setback, vertex, edge_length
            ),
            BlendFailure::DihedralInflection {
                edge,
                station,
                dihedral_deg,
            } => write!(
                f,
                "edge {} dihedral inverts at station {:.3} (angle {:.3}°)",
                edge, station, dihedral_deg
            ),
            BlendFailure::SewGapTooLarge {
                edge,
                gap,
                tolerance,
            } => write!(
                f,
                "sew gap {} at edge {} exceeds tolerance {}",
                gap, edge, tolerance
            ),
            BlendFailure::SpineSolverDiverged {
                edge,
                station,
                residual,
            } => write!(
                f,
                "spine solver diverged at edge {} station {:.3} (residual {})",
                edge, station, residual
            ),
            BlendFailure::VertexBlendUnsupported {
                vertex,
                kind,
                reason,
            } => write!(
                f,
                "vertex {} blend unsupported (kind {:?}): {}",
                vertex, kind, reason
            ),
            BlendFailure::ConflictingBlendKind {
                edge,
                existing_kind,
                requested_kind,
            } => write!(
                f,
                "conflicting blend on edge {}: existing kind {} cannot accept a {} on the same edge or shared corner",
                edge, existing_kind, requested_kind
            ),
            BlendFailure::TopologyViolation { detail } => {
                write!(f, "topology violation: {}", detail)
            }
        }
    }
}

/// Map a structured [`BlendFailure`] onto the legacy
/// [`OperationError`] surface so existing callers keep working
/// without change. The mapping is fixed:
///
/// - **Caller-side parameter problems** → [`OperationError::InvalidInput`].
///   The user / agent picked parameters the geometry cannot satisfy
///   (`RadiusExceedsCurvature`, `SetbackTooLong`, `VertexBlendUnsupported`).
/// - **Geometric / topological obstructions** → [`OperationError::InvalidGeometry`].
///   The input is valid in isolation but the local geometry will not
///   accept a blend at this site (`DihedralInflection`, `SewGapTooLarge`,
///   `TopologyViolation`).
/// - **Numerical convergence failure** → [`OperationError::NumericalError`].
///   The solver did not converge in the allotted iteration budget
///   (`SpineSolverDiverged`).
///
/// Future Phase-2 of Diagnostics-α adds an `OperationError::BlendFailed
/// (Box<BlendFailure>)` variant; this `From` is the bridge that keeps
/// the conversion source-compatible across the upgrade.
impl From<BlendFailure> for OperationError {
    fn from(failure: BlendFailure) -> Self {
        let detail = failure.to_string();
        match failure {
            BlendFailure::RadiusExceedsCurvature { .. }
            | BlendFailure::SetbackTooLong { .. }
            | BlendFailure::VertexBlendUnsupported { .. }
            | BlendFailure::ConflictingBlendKind { .. } => {
                OperationError::InvalidInput {
                    parameter: "blend".to_string(),
                    expected: "geometrically feasible blend parameters".to_string(),
                    received: detail,
                }
            }
            BlendFailure::DihedralInflection { .. }
            | BlendFailure::SewGapTooLarge { .. }
            | BlendFailure::TopologyViolation { .. } => {
                OperationError::InvalidGeometry(detail)
            }
            BlendFailure::SpineSolverDiverged { .. } => {
                OperationError::NumericalError(detail)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_exceeds_curvature_maps_to_invalid_input() {
        let failure = BlendFailure::RadiusExceedsCurvature {
            edge: 7,
            station: 0.42,
            r_requested: 2.0,
            r_max: 1.25,
        };
        let err: OperationError = failure.into();
        match err {
            OperationError::InvalidInput {
                parameter,
                received,
                ..
            } => {
                assert_eq!(parameter, "blend");
                assert!(received.contains("edge 7"));
                assert!(received.contains("r_max=1.25"));
            }
            other => panic!("expected InvalidInput, got {:?}", other),
        }
    }

    #[test]
    fn setback_too_long_maps_to_invalid_input() {
        let failure = BlendFailure::SetbackTooLong {
            vertex: 3,
            setback: 1.5,
            edge_length: 1.0,
        };
        let err: OperationError = failure.into();
        assert!(matches!(err, OperationError::InvalidInput { .. }));
    }

    #[test]
    fn dihedral_inflection_maps_to_invalid_geometry() {
        let failure = BlendFailure::DihedralInflection {
            edge: 11,
            station: 0.5,
            dihedral_deg: 180.0,
        };
        let err: OperationError = failure.into();
        match err {
            OperationError::InvalidGeometry(detail) => {
                assert!(detail.contains("edge 11"));
                assert!(detail.contains("station 0.500"));
            }
            other => panic!("expected InvalidGeometry, got {:?}", other),
        }
    }

    #[test]
    fn sew_gap_maps_to_invalid_geometry() {
        let failure = BlendFailure::SewGapTooLarge {
            edge: 22,
            gap: 0.01,
            tolerance: 1e-6,
        };
        let err: OperationError = failure.into();
        assert!(matches!(err, OperationError::InvalidGeometry(_)));
    }

    #[test]
    fn spine_solver_divergence_maps_to_numerical_error() {
        let failure = BlendFailure::SpineSolverDiverged {
            edge: 5,
            station: 0.7,
            residual: 1e-2,
        };
        let err: OperationError = failure.into();
        match err {
            OperationError::NumericalError(detail) => {
                assert!(detail.contains("spine solver diverged"));
                assert!(detail.contains("edge 5"));
            }
            other => panic!("expected NumericalError, got {:?}", other),
        }
    }

    #[test]
    fn vertex_blend_unsupported_carries_kind_and_reason() {
        let failure = BlendFailure::VertexBlendUnsupported {
            vertex: 9,
            kind: BlendVertexKind::ConvexCorner { degree: 4 },
            reason: VertexBlendUnsupportedReason::DegreeTooHigh { degree: 4 },
        };
        let err: OperationError = failure.into();
        match err {
            OperationError::InvalidInput { received, .. } => {
                assert!(received.contains("vertex 9"));
                assert!(received.contains("DegreeTooHigh") || received.contains("degree 4"));
            }
            other => panic!("expected InvalidInput, got {:?}", other),
        }
    }

    #[test]
    fn topology_violation_carries_detail() {
        let failure = BlendFailure::TopologyViolation {
            detail: "non-manifold edge in fillet chain".to_string(),
        };
        let err: OperationError = failure.into();
        match err {
            OperationError::InvalidGeometry(detail) => {
                assert!(detail.contains("non-manifold edge in fillet chain"));
            }
            other => panic!("expected InvalidGeometry, got {:?}", other),
        }
    }

    #[test]
    fn unsupported_reason_display_distinguishes_variants() {
        // Spot-check that each `VertexBlendUnsupportedReason` carries
        // distinct, non-empty wording so the formatted blend-failure
        // string is actionable.
        let degree = VertexBlendUnsupportedReason::DegreeTooHigh { degree: 5 }.to_string();
        let nonman = VertexBlendUnsupportedReason::NonManifoldNeighbourhood.to_string();
        let mixed = VertexBlendUnsupportedReason::MixedConvexity.to_string();
        let smooth = VertexBlendUnsupportedReason::SmoothVertex.to_string();
        for s in [&degree, &nonman, &mixed, &smooth] {
            assert!(!s.is_empty());
        }
        assert_ne!(degree, nonman);
        assert_ne!(nonman, mixed);
        assert_ne!(mixed, smooth);
        assert!(degree.contains("5"));
    }

    #[test]
    fn from_preserves_display_string_in_payload() {
        // Round-trip: the `Display` output of `BlendFailure` must be
        // exactly the string that lands in the `OperationError`
        // payload. This is the contract callers rely on when grepping
        // logs during the Phase-1 → Phase-2 transition.
        let failure = BlendFailure::RadiusExceedsCurvature {
            edge: 1,
            station: 0.25,
            r_requested: 0.5,
            r_max: 0.3,
        };
        let expected = failure.to_string();
        let err: OperationError = failure.into();
        let actual = match err {
            OperationError::InvalidInput { received, .. } => received,
            other => panic!("expected InvalidInput, got {:?}", other),
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn blend_failure_is_clone_and_eq() {
        // Down-stream code (validation passes, agent diagnostics) may
        // want to compare / clone failures — pin the derives so a
        // future refactor that drops them is caught at compile time.
        let a = BlendFailure::SetbackTooLong {
            vertex: 1,
            setback: 0.5,
            edge_length: 0.4,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    /// JSON contract pin for the typed REST surface: a
    /// `RadiusExceedsCurvature` failure must serialise with
    /// `"type": "RadiusExceedsCurvature"` as the discriminator (internal
    /// tagging on `BlendFailure`) and the inline fields the variant
    /// declares. Agents consuming the api-server error surface
    /// depend on this layout being stable; changing it is a breaking
    /// change to the public contract.
    #[test]
    fn blend_failure_serializes_with_internally_tagged_type_field() {
        let failure = BlendFailure::RadiusExceedsCurvature {
            edge: 7,
            station: 0.42,
            r_requested: 2.0,
            r_max: 1.25,
        };
        let json = serde_json::to_value(&failure).expect("serialize must succeed");
        let obj = json.as_object().expect("serialised failure must be an object");
        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("RadiusExceedsCurvature"),
            "internally-tagged discriminator must surface as the `type` field"
        );
        assert_eq!(obj.get("edge").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(obj.get("r_max").and_then(|v| v.as_f64()), Some(1.25));

        // Round-trip — Deserialize must reconstruct the variant
        // bit-equivalently.
        let restored: BlendFailure =
            serde_json::from_value(json).expect("deserialize must succeed");
        assert_eq!(restored, failure);
    }

    /// `VertexBlendUnsupported` carries a nested `BlendVertexKind`
    /// (from the F2-β blend_graph module). Pin that the nested kind
    /// is serialised in a form a JSON consumer can branch on.
    #[test]
    fn vertex_blend_unsupported_round_trips_nested_kind() {
        let failure = BlendFailure::VertexBlendUnsupported {
            vertex: 9,
            kind: BlendVertexKind::ConvexCorner { degree: 4 },
            reason: VertexBlendUnsupportedReason::DegreeTooHigh { degree: 4 },
        };
        let json = serde_json::to_value(&failure).expect("serialize must succeed");
        let restored: BlendFailure =
            serde_json::from_value(json).expect("deserialize must succeed");
        assert_eq!(restored, failure);
    }

    /// Phase-2 typed-variant: `OperationError::BlendFailed` carries
    /// the structured [`BlendFailure`] verbatim. Call sites that want
    /// agents and the REST surface to receive the taxonomy (rather
    /// than the legacy flattened string) construct this variant
    /// directly. The Phase-1 `From<BlendFailure>` bridge is preserved
    /// for source-compatibility; this test pins that the typed path
    /// is also available and that `Display` formats it sensibly.
    #[test]
    fn blend_failed_variant_carries_typed_payload() {
        let failure = BlendFailure::RadiusExceedsCurvature {
            edge: 7,
            station: 0.42,
            r_requested: 2.0,
            r_max: 1.25,
        };
        let expected_inner = failure.to_string();
        let err = OperationError::BlendFailed(Box::new(failure.clone()));
        // The typed variant preserves the BlendFailure by value so
        // downstream consumers can pattern-match on the taxonomy.
        match &err {
            OperationError::BlendFailed(payload) => {
                assert_eq!(**payload, failure);
            }
            other => panic!("expected BlendFailed, got {:?}", other),
        }
        // Display surfaces the inner failure's wording.
        let rendered = err.to_string();
        assert!(
            rendered.contains(&expected_inner),
            "Display must embed the BlendFailure's own Display; got {:?}",
            rendered
        );
    }

    #[test]
    fn topology_violation_round_trips_detail() {
        // Specific contract: the detail string of TopologyViolation
        // is the *only* free-form payload in the taxonomy and the one
        // most likely to be regex-grepped by callers during the
        // Phase-1 → Phase-2 transition. Preserve it bit-exact.
        let detail = "unexpected non-manifold edge id 42 between F3 and F7";
        let failure = BlendFailure::TopologyViolation {
            detail: detail.to_string(),
        };
        let err: OperationError = failure.into();
        match err {
            OperationError::InvalidGeometry(msg) => {
                assert!(
                    msg.contains(detail),
                    "TopologyViolation detail must survive the conversion verbatim; got {:?}",
                    msg
                );
            }
            other => panic!("expected InvalidGeometry, got {:?}", other),
        }
    }

    // ----------------------------------------------------------------
    // CF-β.2 — pin the new `MixedKindUnsupported` reason and its
    // nested `MixedKindRejectDetail`. Each sub-case maps onto the
    // typed `InvalidInput("blend", …)` surface via the existing
    // `From<BlendFailure>` impl and serialises to its own internally-
    // tagged JSON shape so agents can branch on `reason.type` ==
    // "MixedKindUnsupported" *and* on `reason.detail.type`.
    // ----------------------------------------------------------------

    fn mixed_kind_failure_with(detail: MixedKindRejectDetail) -> BlendFailure {
        BlendFailure::VertexBlendUnsupported {
            vertex: 42,
            kind: BlendVertexKind::ConvexCorner { degree: 3 },
            reason: VertexBlendUnsupportedReason::MixedKindUnsupported {
                existing: VertexBlendKindSet::single(crate::primitives::solid::BlendKind::Chamfer),
                requested: crate::primitives::solid::BlendKind::Fillet,
                detail,
            },
        }
    }

    #[test]
    fn mixed_kind_unsupported_maps_to_invalid_input_and_embeds_detail() {
        let failure = mixed_kind_failure_with(MixedKindRejectDetail::DegreeUnsupported {
            degree: 4,
        });
        let rendered = failure.to_string();
        let err: OperationError = failure.into();
        match err {
            OperationError::InvalidInput {
                parameter,
                received,
                ..
            } => {
                assert_eq!(parameter, "blend");
                assert_eq!(received, rendered);
                assert!(received.contains("vertex 42"));
                assert!(received.contains("mixed-kind corner"));
                assert!(received.contains("degree 4"));
                assert!(received.contains("existing {chamfer}"));
                assert!(received.contains("requested fillet"));
            }
            other => panic!("expected InvalidInput, got {:?}", other),
        }
    }

    #[test]
    fn mixed_kind_reject_detail_display_distinguishes_all_variants() {
        let degree = MixedKindRejectDetail::DegreeUnsupported { degree: 4 }.to_string();
        let displ = MixedKindRejectDetail::MixedDisplacements {
            offsets: vec![0.5],
            radii: vec![0.8],
        }
        .to_string();
        let planar = MixedKindRejectDetail::NonPlanarCap {
            residual: 0.01,
            tolerance: 1e-6,
        }
        .to_string();
        let curved = MixedKindRejectDetail::CurvedAdjacent.to_string();
        let cliff = MixedKindRejectDetail::ConcaveOrCliff.to_string();
        for s in [&degree, &displ, &planar, &curved, &cliff] {
            assert!(!s.is_empty());
        }
        let distinct: std::collections::HashSet<&str> =
            [&degree, &displ, &planar, &curved, &cliff].iter().map(|s| s.as_str()).collect();
        assert_eq!(distinct.len(), 5, "every MixedKindRejectDetail variant must Display distinctly");
        assert!(degree.contains("degree 4"));
        assert!(displ.contains("[0.5]") && displ.contains("[0.8]"));
        assert!(planar.contains("0.01") && planar.contains("1e-6") || planar.contains("0.000001"));
    }

    #[test]
    fn mixed_kind_reject_detail_serde_round_trip_internally_tagged() {
        // Every variant must round-trip and surface its discriminant
        // on the `type` key. Agents reading the typed REST surface
        // branch on this; changing the tag layout breaks the contract.
        let samples = [
            MixedKindRejectDetail::DegreeUnsupported { degree: 4 },
            MixedKindRejectDetail::MixedDisplacements {
                offsets: vec![0.5, 0.5],
                radii: vec![0.8],
            },
            MixedKindRejectDetail::NonPlanarCap {
                residual: 1e-3,
                tolerance: 1e-6,
            },
            MixedKindRejectDetail::CurvedAdjacent,
            MixedKindRejectDetail::ConcaveOrCliff,
        ];
        for sample in &samples {
            let json = serde_json::to_value(sample).expect("MixedKindRejectDetail serialises");
            let type_tag = json["type"].as_str().expect("type tag is a string");
            assert!(
                !type_tag.is_empty(),
                "internally-tagged enum must surface its tag on `type`"
            );
            let back: MixedKindRejectDetail =
                serde_json::from_value(json).expect("round-trip");
            assert_eq!(*sample, back);
        }
    }

    #[test]
    fn mixed_kind_unsupported_nested_serde_round_trip() {
        // The full BlendFailure round-trip: the outer variant is
        // `VertexBlendUnsupported`, the nested reason is
        // `MixedKindUnsupported`, and the nested-nested detail is
        // (say) `MixedDisplacements`. Pin the whole shape so a
        // future flattening doesn't silently break agent parsers.
        let failure = mixed_kind_failure_with(MixedKindRejectDetail::MixedDisplacements {
            offsets: vec![0.5],
            radii: vec![0.8, 0.8],
        });
        let json = serde_json::to_value(&failure).expect("BlendFailure serialises");
        assert_eq!(json["type"], "VertexBlendUnsupported");
        assert_eq!(json["vertex"], 42);
        assert_eq!(json["reason"]["MixedKindUnsupported"]["requested"], "fillet");
        assert_eq!(
            json["reason"]["MixedKindUnsupported"]["detail"]["type"],
            "MixedDisplacements"
        );
        let back: BlendFailure = serde_json::from_value(json).expect("round-trip");
        assert_eq!(failure, back);
    }
}
