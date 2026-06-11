//! Structured import diagnostics for the STEP reader.
//!
//! Every STEP import produces an [`ImportReport`] alongside the
//! resulting `BRepModel`. The report records:
//!
//! - which entities were resolved (counts per category),
//! - which entities were skipped (intentionally — e.g. `STYLED_ITEM`),
//! - which entities were **unsupported** (no handler registered), and
//! - any **warnings** raised during phase-2 dispatch (malformed
//!   parameters, missing back-references, healing fall-backs).
//!
//! Failure semantics, by design:
//!
//! - **Parse-level** failures (the file is not syntactically a STEP
//!   exchange structure) hard-fail the import with
//!   `ExportError::FileReadError`. The report is not produced.
//! - **Handler-level** failures (a handler exists but the record is
//!   malformed) attach a [`Warning`] and continue. Downstream entities
//!   that reference the failed one degrade gracefully — they also
//!   become warnings, never panics.
//! - **Missing handler** for an entity name is logged as
//!   [`Unsupported`]; the entity is simply skipped. No silent
//!   substitution, no fake placeholder geometry.
//!
//! This is the contract that lets the importer accept any STEP file:
//! coverage grows by registering new handlers; until then, an
//! unsupported file produces an honest report rather than a crash or a
//! corrupted import.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Severity of a non-fatal import event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational — entity intentionally skipped (e.g. presentation
    /// layer assignment, color), no geometric impact.
    Info,
    /// Soft failure — a handler ran but produced degraded output (e.g.
    /// edge param-range estimated rather than derived from a NURBS
    /// knot vector). The geometry still imported.
    Warn,
    /// Hard handler failure — the record was malformed or referenced
    /// missing entities. The geometry it represents was dropped from
    /// the resulting BRep.
    Error,
}

/// An entity the importer did not know how to handle.
///
/// `Unsupported` does *not* indicate a bug — it's a structured signal
/// that the file references entities for which no [`crate::formats::step::dispatch::EntityHandler`]
/// is registered. Tier-1 coverage handles ~95% of demo files; the
/// remaining ~5% surface here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unsupported {
    /// STEP entity name (upper-cased), e.g. `"B_SPLINE_SURFACE_WITH_KNOTS"`.
    pub entity: String,
    /// `#N` instance number from the source file.
    pub instance: u64,
    /// Free-form reason from the dispatcher (typically
    /// `"no handler registered for entity X"`).
    pub reason: String,
}

/// A non-fatal event raised by a handler during dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    /// Severity classification — drives client UI presentation.
    pub severity: Severity,
    /// STEP entity name that produced the warning. Empty for
    /// healing-pass or top-level warnings.
    pub entity: String,
    /// `#N` instance number from the source file, when applicable.
    pub instance: Option<u64>,
    /// Human-readable description.
    pub message: String,
}

/// Classification of a healing event applied by the importer.
///
/// STEP files in the wild routinely emit geometry whose curves don't
/// quite reach the declared vertex positions, loops that don't close
/// to floating-point tolerance, or oriented edges whose start/end
/// vertex assignments are inverted relative to the underlying curve
/// parameterisation. Mainstream production importers silently heal
/// these in the obvious way; we heal *and log* so the
/// caller can decide whether the deviations are acceptable for their
/// downstream task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealingKind {
    /// `EDGE_CURVE`'s parametric curve, evaluated at the edge's
    /// parameter range, did not coincide with the named
    /// `VERTEX_POINT` position to within tolerance. We snapped the
    /// curve's reported endpoint to the named vertex position.
    EdgeVertexSnap,
    /// `EDGE_LOOP` did not close to within tolerance — the chain of
    /// oriented edges ended at a different point from where it
    /// started. The loop was emitted anyway; downstream face
    /// validation may reject it.
    LoopNotClosed,
    /// `AXIS2_PLACEMENT_3D.ref_direction` was parallel (or anti-
    /// parallel) to the placement axis. The spec requires
    /// non-parallel; we substituted a synthetic perpendicular X
    /// axis to keep the frame well-defined.
    PlacementAxisDegenerate,
    /// `DIRECTION`'s reported components were the zero vector. We
    /// substituted `+Z` to keep downstream evaluation finite.
    ZeroLengthDirection,
}

/// A structured healing event applied during dispatch. Logged on
/// [`ImportReport::healings`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Healing {
    /// Classification of the heal.
    pub kind: HealingKind,
    /// STEP entity name where the heal was applied.
    pub entity: String,
    /// `#N` instance number from the source file.
    pub instance: u64,
    /// Magnitude of the deviation that triggered the heal, in the
    /// kernel's canonical length unit (mm) for length-typed kinds.
    /// `0.0` for kinds whose deviation is not a scalar
    /// (e.g. `PlacementAxisDegenerate`).
    pub deviation: f64,
    /// Tolerance against which the deviation was judged. Same units
    /// as `deviation`.
    pub tolerance: f64,
}

/// Classification of a manifold-validation issue raised against a
/// `CLOSED_SHELL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifoldKind {
    /// An edge appears in more than two oriented face-uses across
    /// the shell. The shell is non-manifold; boolean operations
    /// against it will produce undefined results.
    NonManifoldEdge,
    /// An edge appears in only one oriented face-use. The shell has
    /// a free boundary and is therefore not closed.
    DanglingEdge,
    /// An edge appears in exactly two face-uses but both have the
    /// same orientation along the edge — the two faces' normals are
    /// inconsistent across this seam.
    OrientationMismatch,
}

/// A manifold-validation event raised against a `CLOSED_SHELL`.
///
/// We emit the shell to the kernel regardless, but downstream
/// callers that rely on a watertight solid (booleans, mass
/// properties, mesh generation for printing) should treat any
/// `ManifoldWarning` on a shell as cause to refuse the operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifoldWarning {
    /// Classification of the issue.
    pub kind: ManifoldKind,
    /// `#N` of the offending `CLOSED_SHELL`.
    pub shell_instance: u64,
    /// Count of edges in the shell that exhibit this issue.
    pub edge_count: usize,
}

/// Aggregate counts of entity outcomes during dispatch.
///
/// Each map is keyed by the STEP entity name (upper-cased). Useful for
/// surfacing "imported 47 planes, 16 NURBS curves, skipped 3 styled
/// items" without re-walking the per-entity Unsupported / Warning
/// vectors.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EntityCounts {
    /// Successfully resolved to a BRep entity.
    pub resolved: BTreeMap<String, u64>,
    /// Intentionally skipped (Severity::Info).
    pub skipped: BTreeMap<String, u64>,
    /// No handler registered.
    pub unsupported: BTreeMap<String, u64>,
    /// Handler ran but raised Warn or Error.
    pub failed: BTreeMap<String, u64>,
}

impl EntityCounts {
    /// Increment the `resolved` count for `entity`.
    pub fn add_resolved(&mut self, entity: &str) {
        *self.resolved.entry(entity.to_string()).or_default() += 1;
    }

    /// Increment the `skipped` count for `entity`.
    pub fn add_skipped(&mut self, entity: &str) {
        *self.skipped.entry(entity.to_string()).or_default() += 1;
    }

    /// Increment the `unsupported` count for `entity`.
    pub fn add_unsupported(&mut self, entity: &str) {
        *self.unsupported.entry(entity.to_string()).or_default() += 1;
    }

    /// Increment the `failed` count for `entity`.
    pub fn add_failed(&mut self, entity: &str) {
        *self.failed.entry(entity.to_string()).or_default() += 1;
    }

    /// Total entities considered (sum across all four buckets).
    pub fn total(&self) -> u64 {
        self.resolved.values().sum::<u64>()
            + self.skipped.values().sum::<u64>()
            + self.unsupported.values().sum::<u64>()
            + self.failed.values().sum::<u64>()
    }
}

/// Outcome of a STEP import.
///
/// Returned from
/// [`crate::formats::step::import_step_to_brep_with_report`]. Callers
/// that only need the BRep can use
/// [`crate::formats::step::import_step_to_brep`], which drops the
/// report; the API server retains it to surface to the client.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    /// `true` if at least one root representation produced a non-empty
    /// solid. `false` when the file parsed but contributed no geometry
    /// (every root entity was unsupported or all handlers failed).
    pub ok: bool,
    /// Per-category entity counts.
    pub counts: EntityCounts,
    /// Entities skipped due to a missing handler.
    pub unsupported: Vec<Unsupported>,
    /// Soft-failure events from handlers and the healing pass.
    pub warnings: Vec<Warning>,
    /// Structured healing events applied during dispatch.
    pub healings: Vec<Healing>,
    /// Manifold-validation issues raised against closed shells.
    pub manifold_warnings: Vec<ManifoldWarning>,
    /// AP detected from the FILE_SCHEMA header, when present.
    pub schema: Option<String>,
    /// Source file unit applied to the resulting BRep (typically
    /// `"mm"`; `"inch"` files are scaled up by 25.4 during import).
    pub source_unit: Option<String>,
    /// Number of root representations (`SHAPE_REPRESENTATION` /
    /// `ADVANCED_BREP_SHAPE_REPRESENTATION`) that resolved to at
    /// least one solid. `0` for files with no root containers, or
    /// for files whose roots only carry placements / mapped items
    /// without a `MANIFOLD_SOLID_BREP`.
    #[serde(default)]
    pub roots_resolved: usize,
    /// Total number of solid ids reachable through some root's
    /// items list. Equal to `model.solids.len()` for well-formed
    /// AP242 files where every solid belongs to exactly one root.
    #[serde(default)]
    pub solids_in_roots: usize,
}

impl ImportReport {
    /// Create an empty report; defaults `ok = false` until at least one
    /// root resolves.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an Unsupported entry and bump the count.
    pub fn push_unsupported(&mut self, entry: Unsupported) {
        self.counts.add_unsupported(&entry.entity);
        self.unsupported.push(entry);
    }

    /// Append a Warning entry and bump the failed count when the
    /// severity is Warn or Error.
    pub fn push_warning(&mut self, warn: Warning) {
        if matches!(warn.severity, Severity::Warn | Severity::Error) && !warn.entity.is_empty() {
            self.counts.add_failed(&warn.entity);
        }
        self.warnings.push(warn);
    }

    /// Record a healing event.
    pub fn push_healing(&mut self, healing: Healing) {
        self.healings.push(healing);
    }

    /// Record a manifold-validation issue.
    pub fn push_manifold_warning(&mut self, mw: ManifoldWarning) {
        self.manifold_warnings.push(mw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_total_sums_all_buckets() {
        let mut c = EntityCounts::default();
        c.add_resolved("LINE");
        c.add_resolved("CIRCLE");
        c.add_skipped("STYLED_ITEM");
        c.add_unsupported("B_SPLINE_SURFACE_WITH_KNOTS");
        c.add_failed("ADVANCED_FACE");
        assert_eq!(c.total(), 5);
        assert_eq!(c.resolved.get("LINE"), Some(&1));
    }

    #[test]
    fn push_unsupported_updates_count() {
        let mut r = ImportReport::new();
        r.push_unsupported(Unsupported {
            entity: "PCURVE".to_string(),
            instance: 42,
            reason: "no handler".to_string(),
        });
        assert_eq!(r.unsupported.len(), 1);
        assert_eq!(r.counts.unsupported.get("PCURVE"), Some(&1));
    }

    #[test]
    fn push_info_warning_does_not_count_as_failed() {
        let mut r = ImportReport::new();
        r.push_warning(Warning {
            severity: Severity::Info,
            entity: "STYLED_ITEM".to_string(),
            instance: Some(7),
            message: "intentionally skipped".to_string(),
        });
        assert_eq!(r.counts.failed.get("STYLED_ITEM"), None);
    }

    #[test]
    fn push_error_warning_increments_failed() {
        let mut r = ImportReport::new();
        r.push_warning(Warning {
            severity: Severity::Error,
            entity: "ADVANCED_FACE".to_string(),
            instance: Some(7),
            message: "missing surface".to_string(),
        });
        assert_eq!(r.counts.failed.get("ADVANCED_FACE"), Some(&1));
    }
}
