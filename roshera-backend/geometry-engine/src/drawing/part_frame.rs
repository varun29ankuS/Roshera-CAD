//! Draw a part in the frame it was designed in.
//!
//! An assembly STEP file places each part with a rigid transform, and the
//! importer applies it, so a part arrives at whatever angle the assembly held
//! it (the GrabCAD U-joint's cross block sits 2.5° off because the joint is
//! flexed). The drawing pipeline projects along the world axes, so a tilted
//! part gets tilted views and -- worse -- its projected widths inked as if
//! they were sizes: a 30 × 20 box drawn at 17° reads 27.90 and 34.54.
//!
//! The importer records each placement ([`BRepModel::solid_placement`]).
//! [`draw_in_part_frame`] undoes it on the caller's OWN copy of the model,
//! so the views are the designer's views and every dimension is a true size.
//! The live model is never touched: both callers (the drawing routes and the
//! STEP probe) build the sheet on a model they own.
//!
//! A part with no placement can still arrive turned: a single-part STEP of an
//! OEM part is often modelled in vehicle position. After any placement is
//! undone the part is squared to its natural frame
//! ([`super::natural_frame`]): the smallest rotation that lays its planar
//! normals and axes on world axes. A part already square is left exactly as
//! modelled, so its author's front view is kept.

use super::natural_frame::{analyse_natural_frame, SQUARE_ENOUGH_DEG};
use crate::math::Matrix4;
use crate::operations::transform::{transform_solid, TransformOptions};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// The frame a sheet was drawn in.
#[derive(Debug, Clone, PartialEq)]
pub enum PartFrame {
    /// No placement was recorded: the part is drawn where it is.
    AsModelled,
    /// The assembly placement was undone: the part is drawn as designed.
    AsDesigned,
    /// A placement was recorded but could not be undone, or the part sits
    /// off square and could not be squared; the part is drawn where it is,
    /// and the reason says why.
    AsPlaced { reason: String },
    /// The part sat turned off the world axes (after any assembly placement
    /// was undone) and was squared to its natural frame: the smallest
    /// rotation that lays its planar normals and axes on world axes.
    Squared {
        /// How far it was turned, degrees.
        tilt_deg: f64,
        /// True when an assembly placement was undone first.
        placement_undone: bool,
    },
}

/// The rigid move a sheet was drawn under, recorded on the sheet
/// ([`super::types::Drawing::drawn_frame`]) so the certificate re-measures the
/// live model in the same frame the sheet was drawn in.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrawnFrame {
    /// Row-major 4×4 rigid transform from the live model to the drawn frame.
    pub rows: [[f64; 4]; 4],
    /// How far the part was turned off square, degrees (0 when it was only
    /// moved back to its design frame).
    pub tilt_deg: f64,
    /// True when an assembly placement was undone.
    pub placement_undone: bool,
}

impl DrawnFrame {
    /// The transform as a matrix.
    pub fn matrix(&self) -> Matrix4 {
        Matrix4::from_rows_array(self.rows)
    }

    fn from_matrix(m: &Matrix4, tilt_deg: f64, placement_undone: bool) -> Self {
        let mut rows = [[0.0; 4]; 4];
        for (r, row) in rows.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = m.get(r, c);
            }
        }
        Self {
            rows,
            tilt_deg,
            placement_undone,
        }
    }
}

/// Move `solid_id` into the frame it is drawn in: back to the frame it was
/// designed in when an assembly placement was recorded, then square to its
/// natural frame when it still sits turned. Mutates `model`: call it on a
/// copy.
///
/// A rigid move cannot change topology, so the pre- and post-validation that
/// `transform_solid` runs by default are off: an imported part with a flaw
/// (an inconsistently oriented face, say) must still be drawable, and the
/// flaw is reported by the import certificate, not hidden here.
///
/// A sheet built on the moved model must carry the returned [`DrawnFrame`]
/// ([`frame_for_drawing`] returns it); this wrapper is for callers that only
/// need to know the frame.
pub fn draw_in_part_frame(model: &mut BRepModel, solid_id: SolidId) -> PartFrame {
    frame_for_drawing(model, solid_id).0
}

/// [`draw_in_part_frame`], also returning the rigid move applied (`None` when
/// the part is drawn where it is). Set it on the sheet built from `model`.
pub fn frame_for_drawing(
    model: &mut BRepModel,
    solid_id: SolidId,
) -> (PartFrame, Option<DrawnFrame>) {
    let undone = match undo_placement(model, solid_id) {
        Ok(undone) => undone,
        Err(reason) => return (PartFrame::AsPlaced { reason }, None),
    };
    // Then square it: a single-part STEP (or a design frame that was itself
    // turned) can still sit off the world axes. A part with no natural frame
    // (no planar or axial face) is drawn as it is.
    let squared = match analyse_natural_frame(model, solid_id) {
        Ok(a) if a.tilt_deg > SQUARE_ENOUGH_DEG => {
            match transform_solid(model, solid_id, a.squaring, rigid_move_options()) {
                Ok(_) => Some((a.tilt_deg, a.squaring)),
                Err(e) => {
                    let reason = format!(
                        "the part sits {:.3} degrees off square and squaring it failed: {e}",
                        a.tilt_deg
                    );
                    // The placement (if any) WAS undone: the sheet is drawn in
                    // that frame and must say so.
                    let frame = undone.map(|inv| DrawnFrame::from_matrix(&inv, 0.0, true));
                    return (PartFrame::AsPlaced { reason }, frame);
                }
            }
        }
        _ => None,
    };
    match (undone, squared) {
        (inv, Some((tilt_deg, squaring))) => {
            let total = match inv {
                Some(inv) => squaring * inv,
                None => squaring,
            };
            (
                PartFrame::Squared {
                    tilt_deg,
                    placement_undone: inv.is_some(),
                },
                Some(DrawnFrame::from_matrix(&total, tilt_deg, inv.is_some())),
            )
        }
        (Some(inv), None) => (
            PartFrame::AsDesigned,
            Some(DrawnFrame::from_matrix(&inv, 0.0, true)),
        ),
        (None, None) => (PartFrame::AsModelled, None),
    }
}

/// A copy of `model` with `solid_id` moved into the frame a sheet was drawn
/// in, for every consumer that re-measures or re-projects the part for that
/// sheet (its certificate, a view added to it): a squared sheet must never
/// receive a view or a measurement from the unsquared part. `None` when the
/// solid is gone or the move fails; the caller reports that, never falls back
/// to the unmoved part silently.
pub fn model_in_drawn_frame(
    model: &BRepModel,
    solid_id: SolidId,
    frame: &DrawnFrame,
) -> Option<BRepModel> {
    model.solids.get(solid_id)?;
    let mut framed = BRepModel::new();
    crate::primitives::snapshot::ModelSnapshot::take(model).restore(&mut framed);
    transform_solid(&mut framed, solid_id, frame.matrix(), rigid_move_options()).ok()?;
    framed.clear_solid_placement(solid_id);
    Some(framed)
}

/// A rigid move cannot change topology; see [`draw_in_part_frame`].
pub(crate) fn rigid_move_options() -> TransformOptions {
    let mut options = TransformOptions::default();
    options.common.validate_before = false;
    options.common.validate_result = false;
    options
}

/// Undo a recorded assembly placement: `Ok(Some(inverse))` when one was
/// undone, `Ok(None)` when none was recorded, `Err(reason)` when it could not
/// be.
fn undo_placement(model: &mut BRepModel, solid_id: SolidId) -> Result<Option<Matrix4>, String> {
    let Some(placement) = model.solid_placement(solid_id).copied() else {
        return Ok(None);
    };
    let inverse = placement
        .inverse()
        .map_err(|e| format!("the recorded assembly placement is singular: {e:?}"))?;
    transform_solid(model, solid_id, inverse, rigid_move_options())
        .map_err(|e| format!("moving the part back to its design frame failed: {e}"))?;
    model.clear_solid_placement(solid_id);
    Ok(Some(inverse))
}

#[cfg(test)]
mod tests {
    use super::{draw_in_part_frame, PartFrame};
    use crate::drawing::dimensioning::standard_drawing_auto;
    use crate::math::{Matrix4, Vector3};
    use crate::operations::transform::{transform_solid, TransformOptions};
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    /// A 40 × 30 × 20 box an assembly turned 17° about X (as the GrabCAD
    /// U-joint holds its cross block) and moved off the origin.
    fn placed_box() -> (BRepModel, u32) {
        let mut model = BRepModel::new();
        let sid = match TopologyBuilder::new(&mut model).create_box_3d(40.0, 30.0, 20.0) {
            Ok(GeometryId::Solid(s)) => s,
            other => panic!("box: {other:?}"),
        };
        let placement = Matrix4::from_translation(&Vector3::new(12.0, -7.0, 31.0))
            * Matrix4::rotation_x(17f64.to_radians());
        transform_solid(&mut model, sid, placement, TransformOptions::default()).expect("place");
        model.set_solid_placement(sid, placement);
        (model, sid)
    }

    fn sheet_values(model: &BRepModel, sid: u32) -> Vec<f64> {
        let dwg = standard_drawing_auto(model, sid, uuid::Uuid::nil()).expect("sheet");
        dwg.views
            .iter()
            .flat_map(|v| v.dimensions.iter())
            .map(|d| d.value)
            .collect()
    }

    #[test]
    fn a_placed_part_is_drawn_with_its_true_sizes() {
        let (mut model, sid) = placed_box();
        assert_eq!(draw_in_part_frame(&mut model, sid), PartFrame::AsDesigned);
        let values = sheet_values(&model, sid);
        for want in [40.0, 30.0, 20.0] {
            assert!(
                values.iter().any(|v| (v - want).abs() < 1e-6),
                "the sheet must carry the box's {want} size; it carries {values:?}"
            );
        }
    }

    #[test]
    fn a_part_with_no_placement_is_left_where_it_is() {
        let mut model = BRepModel::new();
        let sid = match TopologyBuilder::new(&mut model).create_box_3d(40.0, 30.0, 20.0) {
            Ok(GeometryId::Solid(s)) => s,
            other => panic!("box: {other:?}"),
        };
        assert_eq!(draw_in_part_frame(&mut model, sid), PartFrame::AsModelled);
    }
}
