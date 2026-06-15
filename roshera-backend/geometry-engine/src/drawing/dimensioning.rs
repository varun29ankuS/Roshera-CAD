//! Automatic drawing dimensions (#20, slice 1).
//!
//! An engineering drawing is geometry + DIMENSIONS. The projection pipeline
//! draws the edges; this derives the dimension callouts AUTOMATICALLY from the
//! analytic dimension table (`readable::extract_dimensions`) and projects them
//! through the SAME view matrix the edges use, so each callout lands on the
//! feature it measures. Sound by construction: every value is the exact
//! analytic dimension read off a surface/curve — never measured from the
//! rasterised drawing — and each callout names the B-Rep face(s) it spans, so
//! it is recoverable, not decorative.

use super::projection::project_point;
use super::types::ProjectionType;
use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::extract_dimensions;
use serde::{Deserialize, Serialize};

/// A 2D dimension annotation in view-space (mm, pre-scale) — the same space the
/// projected polylines live in, so the SVG/DXF renderer maps both uniformly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension2d {
    /// Stable id carried from the analytic record (the mould handle).
    pub id: String,
    /// "diameter" | "radius" | "length" | "angle" | "extent".
    pub kind: String,
    pub value: f64,
    pub unit: String,
    /// Drawing label, e.g. "Ø20.00", "40.00", "∠30.0°".
    pub label: String,
    /// View-space endpoints of the measured span. For an angle (no linear
    /// span) `a == b` at the feature anchor.
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// B-Rep face ids the dimension spans (empty for whole-part extents).
    pub entities: Vec<u32>,
}

impl Dimension2d {
    /// Projected span length in view-space (mm). ~0 for angle/point callouts,
    /// and for spans that project edge-on in this view (a Z extent in Top).
    pub fn projected_span(&self) -> f64 {
        let dx = self.a[0] - self.b[0];
        let dy = self.a[1] - self.b[1];
        (dx * dx + dy * dy).sqrt()
    }
}

/// Derive the 2D dimension callouts for `solid_id` in the given view.
///
/// Each analytic record carries `(anchor, direction, value, kind)`; the 3D span
/// endpoints follow from the kind:
///   * diameter — across the feature: `anchor → anchor − direction·value`
///   * length / extent — along the axis, centred on the anchor:
///     `anchor ∓ direction·(value/2)`
///   * angle — a point callout at the anchor.
/// Both endpoints are projected through `projection`, so a callout that
/// measures a direction perpendicular to the view collapses to a near-zero
/// span (the caller drops or re-routes those to a view that shows them).
pub fn auto_dimensions(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
) -> Vec<Dimension2d> {
    let mut out = Vec::new();
    for d in extract_dimensions(model, solid_id) {
        let anchor = Point3::new(d.anchor[0], d.anchor[1], d.anchor[2]);
        let dir = Vector3::new(d.direction[0], d.direction[1], d.direction[2]);
        let (p0, p1) = match d.kind.as_str() {
            "diameter" | "radius" => (anchor, anchor - dir * d.value),
            "length" | "extent" => (
                anchor - dir * (d.value * 0.5),
                anchor + dir * (d.value * 0.5),
            ),
            _ => (anchor, anchor),
        };
        let a = project_point(projection, p0);
        let b = project_point(projection, p1);
        out.push(Dimension2d {
            id: d.id,
            kind: d.kind,
            value: d.value,
            unit: d.unit,
            label: d.label,
            a,
            b,
            entities: d.entities,
        });
    }
    out
}

/// Callouts that actually READ in this view: drop the ones whose measured span
/// projects edge-on (e.g. a Z height in Top view), since their line collapses
/// to a point and would clutter without informing. Angles are kept (point
/// callouts). `min_span` is in view-space mm.
pub fn visible_dimensions(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    min_span: f64,
) -> Vec<Dimension2d> {
    auto_dimensions(model, solid_id, projection)
        .into_iter()
        .filter(|d| d.kind == "angle" || d.projected_span() >= min_span)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    fn has(dims: &[Dimension2d], kind: &str, value: f64) -> bool {
        dims.iter()
            .any(|d| d.kind == kind && (d.value - value).abs() < 1e-3)
    }

    #[test]
    fn box_front_view_dimensions_match_built_and_project_true_length() {
        // Box 40(X) × 30(Y) × 20(Z). Front view (camera +Y) shows X→right,
        // Z→up. So the X(40) and Z(20) extents read at TRUE projected length;
        // the Y(30) extent projects edge-on (depth) → near-zero span.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box"));
        let dims = auto_dimensions(&m, b, ProjectionType::Front);
        assert!(has(&dims, "extent", 40.0), "X extent present: {dims:?}");
        assert!(has(&dims, "extent", 30.0), "Y extent present");
        assert!(has(&dims, "extent", 20.0), "Z extent present");

        // Built == drawn: the X extent projects to a ~40mm span in Front.
        let x = dims
            .iter()
            .find(|d| d.kind == "extent" && (d.value - 40.0).abs() < 1e-3)
            .expect("X extent");
        assert!(
            (x.projected_span() - 40.0).abs() < 1e-6,
            "X span {} != 40",
            x.projected_span()
        );
        // The Y (depth) extent projects edge-on in Front → ~0 span.
        let y = dims
            .iter()
            .find(|d| d.kind == "extent" && (d.value - 30.0).abs() < 1e-3)
            .expect("Y extent");
        assert!(
            y.projected_span() < 1e-6,
            "Y depth should project edge-on, got {}",
            y.projected_span()
        );

        // visible_dimensions drops the edge-on Y extent.
        let vis = visible_dimensions(&m, b, ProjectionType::Front, 1.0);
        assert!(has(&vis, "extent", 40.0) && has(&vis, "extent", 20.0));
        assert!(
            !has(&vis, "extent", 30.0),
            "edge-on Y extent dropped from Front"
        );
    }

    #[test]
    fn bored_plate_diameter_callout_is_built_and_recoverable() {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = crate::operations::boolean::boolean_operation(
            &mut m,
            plate,
            bore,
            crate::operations::boolean::BooleanOp::Difference,
            crate::operations::boolean::BooleanOptions::default(),
        )
        .expect("bore");
        // Top view (camera +Z) shows the Ø20 bore across its full diameter.
        let dims = auto_dimensions(&m, part, ProjectionType::Top);
        let dia = dims
            .iter()
            .find(|d| d.kind == "diameter" && (d.value - 20.0).abs() < 1e-3)
            .expect("Ø20 bore callout");
        assert!(
            (dia.projected_span() - 20.0).abs() < 1e-6,
            "Ø20 spans 20mm in Top"
        );
        assert!(
            !dia.entities.is_empty(),
            "diameter callout names its face (recoverable)"
        );
        assert!(
            dia.label.contains("20"),
            "label carries the value: {}",
            dia.label
        );
    }
}
