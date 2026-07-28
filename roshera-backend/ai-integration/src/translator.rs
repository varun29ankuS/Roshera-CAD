/// Translator from voice commands to AI commands
///
/// # Design Rationale
/// - **Why separate translator**: Decouples parsing from execution
/// - **Why preserve context**: Natural text helps improve models
/// - **Performance**: < 1ms translation time
/// - **Business Value**: Easy to add new command types
use crate::commands::VoiceCommand;
use shared_types::{AICommand, ObjectId, TransformType, Vector3D};
use uuid::Uuid;

/// Translate voice command to AI command
pub fn voice_to_ai_command(voice_cmd: VoiceCommand) -> Result<AICommand, TranslationError> {
    match voice_cmd {
        VoiceCommand::Create {
            primitive,
            parameters,
            ..
        } => {
            Ok(AICommand::CreatePrimitive {
                shape_type: primitive,
                parameters,
                position: [0.0, 0.0, 0.0], // Default position, could be extracted from parameters
                material: None,
            })
        }
        VoiceCommand::ActivatePartMaturityWorkflow {
            primitive,
            parameters,
            sketch_plane: _,
            ..
        } => {
            // Translate to same CreatePrimitive command - workflow is handled at a higher level
            Ok(AICommand::CreatePrimitive {
                shape_type: primitive,
                parameters,
                position: [0.0, 0.0, 0.0],
                material: None,
            })
        }
        VoiceCommand::Modify {
            target,
            operation,
            parameters: _,
        } => {
            // Convert operation to transform type
            let transform_type = match operation {
                crate::commands::Operation::Move { x, y, z } => TransformType::Translate {
                    offset: [x as f32, y as f32, z as f32],
                },
                crate::commands::Operation::Rotate { axis, angle } => {
                    let axis_vec = parse_axis(&axis)?;
                    TransformType::Rotate {
                        axis: axis_vec,
                        angle_degrees: angle as f32,
                    }
                }
                crate::commands::Operation::Scale { factor } => TransformType::Scale {
                    factor: [factor as f32, factor as f32, factor as f32],
                },
            };

            Ok(AICommand::Transform {
                object_id: target_to_object_id(target),
                transform_type,
            })
        }
        VoiceCommand::Extrude { .. } => {
            // Extrude has no `AICommand` equivalent (`CreatePrimitive` cannot
            // express "extrude an existing face" and fabricating a 1x1x1 box
            // in its place would silently discard the caller's real intent).
            // Honest refusal until a dedicated extrude command variant exists.
            Err(TranslationError::UnsupportedCommand("Extrude".to_string()))
        }
        VoiceCommand::Query { question, target } => {
            // Simple query analysis
            let analysis_type = if question.contains("mass") || question.contains("volume") {
                shared_types::AnalysisType::MassProperties
            } else if question.contains("area") || question.contains("surface") {
                shared_types::AnalysisType::SurfaceAnalysis
            } else if question.contains("interference") || question.contains("collision") {
                shared_types::AnalysisType::InterferenceCheck
            } else if question.contains("mesh") || question.contains("quality") {
                shared_types::AnalysisType::MeshQuality
            } else {
                shared_types::AnalysisType::Measurements
            };

            Ok(AICommand::Analyze {
                objects: target.map(target_to_object_id).into_iter().collect(),
                analysis_type,
            })
        }
    }
}

/// Parse axis string to vector
fn parse_axis(axis: &str) -> Result<Vector3D, TranslationError> {
    match axis.to_lowercase().as_str() {
        "x" => Ok([1.0, 0.0, 0.0]),
        "y" => Ok([0.0, 1.0, 0.0]),
        "z" => Ok([0.0, 0.0, 1.0]),
        _ => Err(TranslationError::InvalidAxis(axis.to_string())),
    }
}

/// Convert internal UUID to ObjectId
fn target_to_object_id(target: Uuid) -> ObjectId {
    target
}

/// Translation errors
#[derive(Debug, thiserror::Error)]
pub enum TranslationError {
    #[error("Invalid axis: {0}")]
    InvalidAxis(String),

    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("Unsupported voice command: {0}")]
    UnsupportedCommand(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared_types::{PrimitiveType, ShapeParameters};

    #[test]
    fn test_create_translation() {
        let voice_cmd = VoiceCommand::Create {
            primitive: PrimitiveType::Sphere,
            parameters: ShapeParameters::sphere_params(5.0),
            natural_text: "create a sphere".to_string(),
        };

        let ai_cmd = voice_to_ai_command(voice_cmd).unwrap();
        match ai_cmd {
            AICommand::CreatePrimitive { shape_type, .. } => {
                assert_eq!(shape_type, PrimitiveType::Sphere);
            }
            _ => panic!("Wrong command type"),
        }
    }

    /// H1 RED: translating a voice Extrude must be an honest refusal, not a
    /// fabricated 1x1x1 box. Extrude has no CreatePrimitive semantics, so
    /// `voice_to_ai_command` must return `Err` here, never
    /// `Ok(AICommand::CreatePrimitive { .. })`.
    #[test]
    fn test_extrude_translation_is_not_implemented() {
        let voice_cmd = VoiceCommand::Extrude {
            target: None,
            face_index: Some(0),
            direction: None,
            distance: Some(5.0),
            natural_text: "extrude this face by 5".to_string(),
        };

        let result = voice_to_ai_command(voice_cmd);

        match result {
            Ok(AICommand::CreatePrimitive { .. }) => {
                panic!("Extrude must not be translated into a fabricated CreatePrimitive")
            }
            Ok(_) => panic!("Extrude must not silently succeed as any other command"),
            Err(_) => {} // honest refusal — expected
        }
    }
}
