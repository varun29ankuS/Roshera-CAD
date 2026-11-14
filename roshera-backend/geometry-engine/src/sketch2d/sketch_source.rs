//! A sketch's SOURCE, in a form a history event can carry (Varun
//! 2026-11-14: "no way to edit the sketch").
//!
//! An extrude used to record only its finished outline, so a sketch could
//! not be reopened -- nor survive a restart. [`SketchSource`] is the whole
//! sketch: every entity with its id, flags and shared-point links, in
//! insertion order, and every constraint naming them, in their order.
//! [`super::sketch::Sketch::to_source`] writes it out;
//! [`super::sketch::Sketch::from_source`] reads it back as the same
//! sketch -- same ids, same links, same constraints -- so the sketch the
//! user reopens is the one they drew, and it solves to the same place.
//!
//! Ids are preserved because constraints name entities by id. A source
//! whose constraints name an entity it does not hold is REFUSED, never
//! restored with the constraint silently dropped.

use super::arc2d::Arc2dId;
use super::circle2d::Circle2dId;
use super::constraints::Constraint;
use super::ellipse2d::Ellipse2dId;
use super::line2d::{Line2dId, LineGeometry};
use super::point2d::Point2dId;
use super::polyline2d::Polyline2dId;
use super::rectangle2d::Rectangle2dId;
use super::spline2d::Spline2dId;
use super::{Arc2d, Circle2d, Ellipse2d, Point2d, Polyline2d, Rectangle2d, Spline2d};
use serde::{Deserialize, Serialize};

/// Format version of [`SketchSource`]; bumped on any breaking change.
pub const SKETCH_SOURCE_VERSION: u32 = 1;

/// One sketch entity as written into history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceEntity {
    /// A point; `fixed` pins both of its freedoms.
    Point {
        id: Point2dId,
        position: Point2d,
        fixed: bool,
        construction: bool,
    },
    /// A line; `endpoints` are the point entities a segment is built on.
    Line {
        id: Line2dId,
        geometry: LineGeometry,
        endpoints: Option<(Point2dId, Point2dId)>,
        construction: bool,
    },
    /// An arc, with its shared end and centre points when it has them.
    Arc {
        id: Arc2dId,
        arc: Arc2d,
        endpoints: Option<(Point2dId, Point2dId)>,
        center_point: Option<Point2dId>,
        construction: bool,
    },
    /// A circle, with its shared centre point when it has one.
    Circle {
        id: Circle2dId,
        circle: Circle2d,
        center_point: Option<Point2dId>,
        construction: bool,
    },
    /// A rectangle.
    Rectangle {
        id: Rectangle2dId,
        rectangle: Rectangle2d,
        construction: bool,
    },
    /// An ellipse.
    Ellipse {
        id: Ellipse2dId,
        ellipse: Ellipse2d,
        construction: bool,
    },
    /// A spline, with its shared control points when it has them.
    Spline {
        id: Spline2dId,
        spline: Spline2d,
        control_point_ids: Option<Vec<Point2dId>>,
        construction: bool,
    },
    /// A polyline.
    Polyline {
        id: Polyline2dId,
        polyline: Polyline2d,
        construction: bool,
    },
}

/// The whole sketch: entities in insertion order, constraints in their order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchSource {
    /// [`SKETCH_SOURCE_VERSION`] at the time it was written.
    pub version: u32,
    /// The sketch's name.
    pub name: String,
    /// Every entity, in insertion order.
    pub entities: Vec<SourceEntity>,
    /// Every constraint, in insertion order.
    pub constraints: Vec<Constraint>,
}
