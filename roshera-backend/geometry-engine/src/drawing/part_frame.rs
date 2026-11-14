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
    /// A placement was recorded but could not be undone; the part is drawn
    /// where the assembly put it, and the reason says why.
    AsPlaced { reason: String },
}

/// Move `solid_id` back to the frame it was designed in, if an assembly
/// placement was recorded for it. Mutates `model`: call it on a copy.
///
/// A rigid move cannot change topology, so the pre- and post-validation that
/// `transform_solid` runs by default are off: an imported part with a flaw
/// (an inconsistently oriented face, say) must still be drawable, and the
/// flaw is reported by the import certificate, not hidden here.
pub fn draw_in_part_frame(model: &mut BRepModel, solid_id: SolidId) -> PartFrame {
    let Some(placement) = model.solid_placement(solid_id).copied() else {
        return PartFrame::AsModelled;
    };
    let inverse = match placement.inverse() {
        Ok(m) => m,
        Err(e) => {
            return PartFrame::AsPlaced {
                reason: format!("the recorded assembly placement is singular: {e:?}"),
            }
        }
    };
    let mut options = TransformOptions::default();
    options.common.validate_before = false;
    options.common.validate_result = false;
    match transform_solid(model, solid_id, inverse, options) {
        Ok(_) => {
            model.clear_solid_placement(solid_id);
            PartFrame::AsDesigned
        }
        Err(e) => PartFrame::AsPlaced {
            reason: format!("moving the part back to its design frame failed: {e}"),
        },
    }
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
