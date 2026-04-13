//! Operation implementations for CAD operations

mod boolean;
mod brep_helpers;
mod chamfer;
mod common;
mod create_primitive;
mod create_sketch;
mod delete;
mod extrude;
mod fillet;
mod loft;
mod modify;
mod pattern;
mod revolve;
mod sweep;
mod transform;

pub use boolean::{BooleanDifferenceOp, BooleanIntersectionOp, BooleanUnionOp};
pub use chamfer::ChamferOp;
pub use create_primitive::CreatePrimitiveOp;
pub use create_sketch::CreateSketchOp;
pub use delete::DeleteOp;
pub use extrude::ExtrudeOp;
pub use fillet::FilletOp;
pub use loft::LoftOp;
pub use modify::ModifyOp;
pub use pattern::PatternOp;
pub use revolve::RevolveOp;
pub use sweep::SweepOp;
pub use transform::TransformOp;

// Re-export helpers
pub use brep_helpers::BRepModelExt;

use crate::execution::ExecutionEngine;

/// Register all operation implementations
pub fn register_all_operations(engine: &ExecutionEngine) {
    // Creation operations
    engine.register_operation(CreateSketchOp);
    engine.register_operation(CreatePrimitiveOp);

    // Modification operations
    engine.register_operation(ExtrudeOp);
    engine.register_operation(RevolveOp);
    engine.register_operation(LoftOp);
    engine.register_operation(SweepOp);

    // Boolean operations
    engine.register_operation(BooleanUnionOp);
    engine.register_operation(BooleanIntersectionOp);
    engine.register_operation(BooleanDifferenceOp);

    // Feature operations
    engine.register_operation(FilletOp);
    engine.register_operation(ChamferOp);

    // Pattern operations
    engine.register_operation(PatternOp);

    // Transform operations
    engine.register_operation(TransformOp);

    // Entity management
    engine.register_operation(DeleteOp);
    engine.register_operation(ModifyOp);
}
