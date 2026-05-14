//! Trim-curve carrier for the F7 trim/sew pipeline.
//!
//! A [`TrimCurve`] is the wire shape that fillet/chamfer/boolean ship
//! into [`crate::operations::imprint::imprint_curves_on_face`] when
//! they need to cut a face with a rail/intersection curve. Today the
//! kernel has the curves (rolling-ball rails, spine rails, boolean
//! intersection curves) and the imprint primitive, but no carrier
//! that records *which face* a curve trims, *which side* of the cut
//! survives, and *what parameter ranges* on the trimming curve are
//! actually live (intersection curves can re-enter a face multiple
//! times — each entry/exit is its own range).
//!
//! ## F7 invariants this carrier enforces
//!
//! - **Curve and face are siblings of the same `BRepModel`.** A
//!   `TrimCurve` is meaningless outside the model that owns both the
//!   `CurveId` and the `FaceId`. Downstream slices feed it directly
//!   into `imprint_curves_on_face` which takes `&mut BRepModel`, so
//!   the carrier itself is purely data — no model references.
//! - **Ranges are sorted, non-overlapping, half-open `[a, b)`** on the
//!   trimming curve's own parameterisation. An empty `ranges` vector
//!   means "the entire valid parameter range of the curve trims this
//!   face" — used by closed-loop rails (fillet's circular cap trims)
//!   where there is no entry/exit pair to enumerate.
//! - **Side classifies the partition geometrically, not loop-wise.**
//!   `Keep` is the partition of the face the rolling ball / boolean
//!   result occupies; `Discard` is what the F7-γ slice will detach
//!   from the shell; `Boundary` is reserved for the (degenerate)
//!   case where the trim curve lies on the existing loop boundary
//!   and the imprint is a no-op (coincident edge merge).
//!
//! Per project hard rule #2 ("production-grade only"): this module
//! ships pure data carriers and constructors. The actual trim
//! application lives in F7-β (`fillet::update_adjacent_faces` wiring).
//! No `Ok(())` stubs here.

use crate::primitives::{curve::CurveId, face::FaceId};

/// Which side of the trim curve survives in the post-trim face.
///
/// The classification is geometric and stable under loop reversal:
/// `Keep` is always the partition adjacent to the blend / boolean
/// result solid, regardless of which orientation the surrounding
/// loop happens to traverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrimSide {
    /// Partition that remains as part of the host face after the
    /// imprint. For fillet, this is the side of the rolling-ball
    /// rail facing *away* from the swept-out material.
    Keep,
    /// Partition that the F7-γ slice will detach from the shell and
    /// stitch into the new blend / boolean face. For fillet, this is
    /// the side of the rail covered by the rolling ball.
    Discard,
    /// Degenerate case: the trim curve coincides with an existing
    /// loop edge to within edge tolerance. The imprint pass treats
    /// this as a coincident-edge merge, not a face split.
    Boundary,
}

/// A single trim-curve carrier: one curve trimming one face with a
/// known side classification.
///
/// ```text
///     curve   ranges      side
///       │       │           │
///       ▼       ▼           ▼
///   ┌───────┐  ┌──────┐  ┌────────┐
///   │CurveId│  │[(a,b)│  │TrimSide│
///   └───────┘  │ ...] │  └────────┘
///              └──────┘
///                │
///                ▼
///         on this FaceId
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TrimCurve {
    /// The trimming curve. Must resolve in the same `BRepModel` that
    /// owns `on_face`.
    pub curve_id: CurveId,
    /// The face being trimmed.
    pub on_face: FaceId,
    /// Sorted, non-overlapping parameter ranges on the trimming
    /// curve that are active for this trim. Empty means "entire
    /// curve" (closed-loop trim).
    pub ranges: Vec<(f64, f64)>,
    /// Which partition survives.
    pub side: TrimSide,
}

impl TrimCurve {
    /// Construct a trim with explicit ranges.
    ///
    /// Returns `None` when any range is degenerate (`a >= b`) or the
    /// ranges are not sorted-disjoint — this is a hard precondition
    /// of `imprint::imprint_curves_on_face` and we surface it at
    /// construction so callers do not silently feed it bad input.
    #[inline]
    pub fn try_new(
        curve_id: CurveId,
        on_face: FaceId,
        ranges: Vec<(f64, f64)>,
        side: TrimSide,
    ) -> Option<Self> {
        if !Self::ranges_well_formed(&ranges) {
            return None;
        }
        Some(Self {
            curve_id,
            on_face,
            ranges,
            side,
        })
    }

    /// Construct a trim that covers the curve's entire valid
    /// parameter range. Used by closed-loop rail trims (fillet
    /// cap-rim circle on a cylindrical end face) where the rail
    /// returns to its start without crossing the host face boundary.
    #[inline]
    pub fn full_range(curve_id: CurveId, on_face: FaceId, side: TrimSide) -> Self {
        Self {
            curve_id,
            on_face,
            ranges: Vec::new(),
            side,
        }
    }

    /// `true` when the trim covers the entire curve (no ranges
    /// enumerated). See `full_range` for the canonical constructor.
    #[inline]
    pub fn is_full_range(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Total parameter span across all enumerated ranges. Returns
    /// `0.0` for a full-range trim — callers asking "how much
    /// parameter does this trim cover" against a full-range trim
    /// should consult the curve directly.
    #[inline]
    pub fn covered_span(&self) -> f64 {
        self.ranges.iter().map(|(a, b)| b - a).sum()
    }

    /// Validate range well-formedness: sorted ascending, disjoint,
    /// strictly positive width.
    fn ranges_well_formed(ranges: &[(f64, f64)]) -> bool {
        let mut prev_end = f64::NEG_INFINITY;
        for &(a, b) in ranges {
            if !(a < b) {
                return false;
            }
            if a < prev_end {
                return false;
            }
            prev_end = b;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_new_accepts_sorted_disjoint_ranges() {
        let t = TrimCurve::try_new(
            7,
            3,
            vec![(0.0, 0.25), (0.5, 0.75)],
            TrimSide::Discard,
        );
        assert!(t.is_some());
        let t = t.unwrap_or_else(|| unreachable!("just asserted Some"));
        assert_eq!(t.curve_id, 7);
        assert_eq!(t.on_face, 3);
        assert_eq!(t.side, TrimSide::Discard);
        assert_eq!(t.ranges.len(), 2);
        assert!(!t.is_full_range());
        assert!((t.covered_span() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn try_new_rejects_degenerate_range() {
        assert!(TrimCurve::try_new(0, 0, vec![(0.3, 0.3)], TrimSide::Keep).is_none());
        assert!(TrimCurve::try_new(0, 0, vec![(0.5, 0.2)], TrimSide::Keep).is_none());
    }

    #[test]
    fn try_new_rejects_overlapping_ranges() {
        assert!(
            TrimCurve::try_new(0, 0, vec![(0.0, 0.4), (0.3, 0.7)], TrimSide::Keep).is_none()
        );
    }

    #[test]
    fn try_new_rejects_unsorted_ranges() {
        assert!(
            TrimCurve::try_new(0, 0, vec![(0.5, 0.7), (0.0, 0.3)], TrimSide::Keep).is_none()
        );
    }

    #[test]
    fn try_new_accepts_touching_ranges() {
        // Touching at endpoints is allowed — (0,a) and (a,b) are
        // disjoint half-open intervals.
        let t = TrimCurve::try_new(0, 0, vec![(0.0, 0.3), (0.3, 0.6)], TrimSide::Keep);
        assert!(t.is_some());
    }

    #[test]
    fn full_range_constructor() {
        let t = TrimCurve::full_range(5, 11, TrimSide::Boundary);
        assert_eq!(t.curve_id, 5);
        assert_eq!(t.on_face, 11);
        assert_eq!(t.side, TrimSide::Boundary);
        assert!(t.is_full_range());
        assert_eq!(t.ranges.len(), 0);
        assert_eq!(t.covered_span(), 0.0);
    }

    #[test]
    fn trim_side_variants_are_distinct() {
        // Pin the enum cardinality so future additions are deliberate.
        let sides = [TrimSide::Keep, TrimSide::Discard, TrimSide::Boundary];
        for (i, a) in sides.iter().enumerate() {
            for (j, b) in sides.iter().enumerate() {
                assert_eq!(a == b, i == j);
            }
        }
    }
}
