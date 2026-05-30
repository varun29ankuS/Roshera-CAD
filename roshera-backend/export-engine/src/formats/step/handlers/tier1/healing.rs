//! Geometry healing helpers used by tier-1 topology handlers.
//!
//! Real STEP files routinely contain small geometric inconsistencies
//! that a production importer must repair silently while staying
//! observable:
//!
//! - **Vertex-curve snap.** `EDGE_CURVE`'s parametric curve, evaluated
//!   at the edge's parameter range, may not coincide with the named
//!   `VERTEX_POINT` to within the source-declared tolerance. We snap
//!   the curve's reported endpoint to the named vertex and log a
//!   [`HealingKind::EdgeVertexSnap`].
//!
//! - **Loop closure.** `EDGE_LOOP` should be a closed chain — last
//!   edge's end-vertex equals the first edge's start-vertex. In the
//!   wild, the closure can fail by sub-mm amounts (CAD systems use
//!   independent fp paths to compute endpoints of adjacent curves).
//!   We compute the gap and emit a [`HealingKind::LoopNotClosed`] if
//!   it exceeds tolerance.
//!
//! - **Placement degeneracy.** `AXIS2_PLACEMENT_3D.ref_direction`
//!   should not be parallel to `axis`. If it is, we synthesise a
//!   perpendicular reference using a deterministic rule (the axis
//!   component with the smallest absolute value as the seed) and log
//!   a [`HealingKind::PlacementAxisDegenerate`].
//!
//! - **Zero-length direction.** `DIRECTION` with all components zero
//!   is ill-defined; we substitute `+Z` and log
//!   [`HealingKind::ZeroLengthDirection`].
//!
//! Every helper here is a pure function over its inputs plus a side
//! effect on `report` — no mutation of the model or caches. The
//! handler decides what to do with the returned value.

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Healing, HealingKind},
};

/// Tolerance for detecting parallelism between two unit vectors via
/// |dot| ≥ 1 − ε. ε is chosen comfortably above f64 round-off for
/// values produced by normalising a non-trivially-perpendicular pair.
const PARALLEL_DOT_EPSILON: f64 = 1e-10;

/// Tolerance below which a vector's magnitude is considered zero.
const ZERO_MAGNITUDE_EPSILON: f64 = 1e-12;

/// 3-D Euclidean distance between two points.
#[inline]
fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// 3-D dot product.
#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// 3-D cross product.
#[inline]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// 3-D vector magnitude.
#[inline]
fn mag(v: [f64; 3]) -> f64 {
    dot(v, v).sqrt()
}

/// Normalise a vector. Returns `None` when `v` is sub-tolerance.
#[inline]
pub fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let m = mag(v);
    if m < ZERO_MAGNITUDE_EPSILON {
        None
    } else {
        Some([v[0] / m, v[1] / m, v[2] / m])
    }
}

/// Normalise a candidate direction, healing zero-length to `+Z` and
/// logging a [`HealingKind::ZeroLengthDirection`] when triggered.
pub fn heal_direction(
    raw: [f64; 3],
    entity: &str,
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> [f64; 3] {
    match normalize(raw) {
        Some(unit) => unit,
        None => {
            ctx.report.push_healing(Healing {
                kind: HealingKind::ZeroLengthDirection,
                entity: entity.to_string(),
                instance,
                deviation: mag(raw),
                tolerance: ZERO_MAGNITUDE_EPSILON,
            });
            [0.0, 0.0, 1.0]
        }
    }
}

/// Build a right-handed orthonormal frame from `axis` and
/// `ref_direction`, projecting `ref_direction` into the plane
/// perpendicular to `axis`. When the projection collapses (because
/// `ref_direction` is parallel to `axis`), synthesise a perpendicular
/// reference using the canonical-axis-with-smallest-axis-dot rule and
/// log [`HealingKind::PlacementAxisDegenerate`].
///
/// Returns `(x, y, z)` axes; all unit length, right-handed.
pub fn build_axis_frame(
    axis: [f64; 3],
    ref_dir: [f64; 3],
    entity: &str,
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let z = heal_direction(axis, entity, instance, ctx);
    let ref_unit = heal_direction(ref_dir, entity, instance, ctx);

    // Project ref onto the plane normal to z: x_raw = ref - (ref·z) z.
    let proj_scalar = dot(ref_unit, z);
    let x_raw = [
        ref_unit[0] - proj_scalar * z[0],
        ref_unit[1] - proj_scalar * z[1],
        ref_unit[2] - proj_scalar * z[2],
    ];

    let x = if let Some(unit) = normalize(x_raw) {
        unit
    } else {
        // ref parallel to z — synthesise.
        ctx.report.push_healing(Healing {
            kind: HealingKind::PlacementAxisDegenerate,
            entity: entity.to_string(),
            instance,
            deviation: dot(ref_unit, z).abs(),
            tolerance: PARALLEL_DOT_EPSILON,
        });
        synthesize_perpendicular(z)
    };

    let y = cross(z, x);
    let y = normalize(y).unwrap_or([0.0, 1.0, 0.0]);
    (x, y, z)
}

/// Pick a unit vector perpendicular to `z` using a deterministic
/// canonical-axis rule. Used to repair a degenerate
/// `AXIS2_PLACEMENT_3D.ref_direction`.
fn synthesize_perpendicular(z: [f64; 3]) -> [f64; 3] {
    let ax = z[0].abs();
    let ay = z[1].abs();
    let az = z[2].abs();
    let candidate = if ax <= ay && ax <= az {
        [1.0, 0.0, 0.0]
    } else if ay <= az {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let proj = dot(candidate, z);
    let raw = [
        candidate[0] - proj * z[0],
        candidate[1] - proj * z[1],
        candidate[2] - proj * z[2],
    ];
    // `candidate` is the axis with the smallest |z·candidate|, so
    // the projection cannot be ±1, and `raw` is non-zero.
    normalize(raw).unwrap_or([1.0, 0.0, 0.0])
}

/// Check whether `curve_endpoint` deviates from `vertex_position` by
/// more than `tolerance`. Returns the deviation. When the deviation
/// exceeds tolerance, a [`HealingKind::EdgeVertexSnap`] event is
/// recorded — the caller is expected to use `vertex_position` and
/// drop `curve_endpoint`.
pub fn check_edge_vertex_snap(
    curve_endpoint: [f64; 3],
    vertex_position: [f64; 3],
    tolerance: f64,
    entity: &str,
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> f64 {
    let dev = dist(curve_endpoint, vertex_position);
    if dev > tolerance {
        ctx.report.push_healing(Healing {
            kind: HealingKind::EdgeVertexSnap,
            entity: entity.to_string(),
            instance,
            deviation: dev,
            tolerance,
        });
    }
    dev
}

/// Verify that a sequence of (start, end) vertex positions forms a
/// closed chain — `chain[i].end == chain[i+1].start` and the last
/// `end` equals the first `start`, all to within `tolerance`.
///
/// Returns the maximum deviation observed. When the closure gap (the
/// distance between last end and first start) exceeds `tolerance`,
/// a [`HealingKind::LoopNotClosed`] is recorded.
///
/// The chain is **not** required to be non-empty; an empty chain
/// returns `0.0` and emits no healing.
pub fn check_loop_closure(
    chain: &[([f64; 3], [f64; 3])],
    tolerance: f64,
    entity: &str,
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> f64 {
    if chain.is_empty() {
        return 0.0;
    }
    let mut worst = 0.0_f64;
    for w in chain.windows(2) {
        let gap = dist(w[0].1, w[1].0);
        worst = worst.max(gap);
    }
    let last = chain[chain.len() - 1].1;
    let first = chain[0].0;
    let closure_gap = dist(last, first);
    worst = worst.max(closure_gap);

    if closure_gap > tolerance {
        ctx.report.push_healing(Healing {
            kind: HealingKind::LoopNotClosed,
            entity: entity.to_string(),
            instance,
            deviation: closure_gap,
            tolerance,
        });
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::diagnostics::ImportReport;
    use geometry_engine::primitives::topology_builder::BRepModel;

    fn ctx_with_report<'a>(
        model: &'a mut BRepModel,
        report: &'a mut ImportReport,
    ) -> ImportContext<'a> {
        let mut c = ImportContext::new(model, report);
        c.default_tolerance = 1e-6;
        c
    }

    #[test]
    fn normalize_unit_vector() {
        let n = normalize([3.0, 0.0, 0.0]).unwrap();
        assert!((n[0] - 1.0).abs() < 1e-15);
        assert_eq!(n[1], 0.0);
    }

    #[test]
    fn normalize_zero_returns_none() {
        assert!(normalize([0.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn heal_direction_substitutes_for_zero() {
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ctx_with_report(&mut model, &mut report);
        let d = heal_direction([0.0, 0.0, 0.0], "DIRECTION", 7, &mut ctx);
        assert_eq!(d, [0.0, 0.0, 1.0]);
        assert_eq!(ctx.report.healings.len(), 1);
        assert_eq!(
            ctx.report.healings[0].kind,
            HealingKind::ZeroLengthDirection
        );
    }

    #[test]
    fn build_axis_frame_orthonormal_for_canonical_input() {
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ctx_with_report(&mut model, &mut report);
        let (x, y, z) = build_axis_frame(
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            "AXIS2_PLACEMENT_3D",
            1,
            &mut ctx,
        );
        assert!((mag(x) - 1.0).abs() < 1e-12);
        assert!((mag(y) - 1.0).abs() < 1e-12);
        assert!((mag(z) - 1.0).abs() < 1e-12);
        assert!(dot(x, y).abs() < 1e-12);
        assert!(dot(y, z).abs() < 1e-12);
        assert!(dot(z, x).abs() < 1e-12);
        // Right-handed: z == x × y.
        let xy = cross(x, y);
        assert!((xy[0] - z[0]).abs() < 1e-12);
        assert!((xy[1] - z[1]).abs() < 1e-12);
        assert!((xy[2] - z[2]).abs() < 1e-12);
        assert!(ctx.report.healings.is_empty());
    }

    #[test]
    fn build_axis_frame_heals_parallel_ref_direction() {
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ctx_with_report(&mut model, &mut report);
        let (x, _y, z) = build_axis_frame(
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0], // parallel — degenerate
            "AXIS2_PLACEMENT_3D",
            5,
            &mut ctx,
        );
        assert!(dot(x, z).abs() < 1e-12);
        assert!((mag(x) - 1.0).abs() < 1e-12);
        assert!(ctx
            .report
            .healings
            .iter()
            .any(|h| h.kind == HealingKind::PlacementAxisDegenerate));
    }

    #[test]
    fn check_edge_vertex_snap_logs_when_over_tolerance() {
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ctx_with_report(&mut model, &mut report);
        let dev = check_edge_vertex_snap(
            [1.0, 0.0, 0.0],
            [1.001, 0.0, 0.0],
            1e-6,
            "EDGE_CURVE",
            12,
            &mut ctx,
        );
        assert!((dev - 0.001).abs() < 1e-12);
        assert_eq!(ctx.report.healings.len(), 1);
        assert_eq!(ctx.report.healings[0].kind, HealingKind::EdgeVertexSnap);
    }

    #[test]
    fn check_edge_vertex_snap_silent_when_within_tolerance() {
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ctx_with_report(&mut model, &mut report);
        check_edge_vertex_snap(
            [1.0, 0.0, 0.0],
            [1.0 + 1e-9, 0.0, 0.0],
            1e-6,
            "EDGE_CURVE",
            12,
            &mut ctx,
        );
        assert!(ctx.report.healings.is_empty());
    }

    #[test]
    fn check_loop_closure_closed_chain_silent() {
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ctx_with_report(&mut model, &mut report);
        // Unit square: (0,0,0)→(1,0,0)→(1,1,0)→(0,1,0)→back.
        let chain = vec![
            ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]),
            ([1.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 0.0]),
        ];
        let worst = check_loop_closure(&chain, 1e-6, "EDGE_LOOP", 1, &mut ctx);
        assert!(worst < 1e-12);
        assert!(ctx.report.healings.is_empty());
    }

    #[test]
    fn check_loop_closure_open_chain_logs() {
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ctx_with_report(&mut model, &mut report);
        let chain = vec![
            ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]),
            ([1.0, 1.0, 0.0], [0.5, 0.5, 0.0]), // gap to (0,0,0)
        ];
        let worst = check_loop_closure(&chain, 1e-6, "EDGE_LOOP", 1, &mut ctx);
        assert!(worst > 0.5);
        assert!(ctx
            .report
            .healings
            .iter()
            .any(|h| h.kind == HealingKind::LoopNotClosed));
    }
}
