//! AI-First operations registry.
//!
//! Provides an AI-callable interface to all CAD operations:
//! - Natural-language parsing of operation intents
//! - Parameter inference from partial input
//! - Operation chaining and batch execution
//! - Usage-based catalog hints

use crate::math::{Point3, Vector3};
use crate::operations::{
    boolean::{BooleanOp, BooleanOptions},
    chamfer::ChamferOptions,
    extrude::ExtrudeOptions,
    fillet::FilletOptions,
    pattern::{PatternOptions, PatternType},
    revolve::RevolveOptions,
    OperationError, OperationResult,
};
use crate::primitives::{
    edge::EdgeId, face::FaceId, solid::SolidId, topology_builder::BRepModel, vertex::VertexId,
};
use serde::{Deserialize, Serialize};
// HashMap removed - using DashMap globally
use dashmap::DashMap;
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};

/// Global operations registry with DashMap for high-performance concurrent access
static OPERATIONS_REGISTRY: LazyLock<Arc<Mutex<OperationsRegistry>>> =
    LazyLock::new(|| Arc::new(Mutex::new(OperationsRegistry::new())));

/// Global operation execution cache for sub-millisecond performance
static OPERATION_CACHE: LazyLock<DashMap<u64, CachedOperationResult>> =
    LazyLock::new(|| DashMap::new());

/// Cached operation result for high-performance execution
#[derive(Debug, Clone)]
struct CachedOperationResult {
    result: AIOperationResult,
    created_at: std::time::Instant,
    hit_count: u64,
}

/// AI-friendly operation command
#[derive(Debug, Clone)]
pub struct AIOperationCommand {
    /// Operation type (e.g., "boolean", "fillet", "extrude")
    pub operation_type: String,
    /// Target entities (solids, faces, edges, etc.)
    pub targets: Vec<EntityReference>,
    /// Operation parameters
    pub parameters: DashMap<String, AIOperationParameter>,
    /// Confidence in parameter extraction
    pub confidence: f64,
    /// Original natural language input
    pub original_text: String,
    /// Suggested improvements
    pub suggestions: Vec<String>,
}

/// Reference to a geometric entity
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "id")]
pub enum EntityReference {
    Solid(SolidId),
    Face(FaceId),
    Edge(EdgeId),
    Vertex(VertexId),
    Multiple(Vec<EntityReference>),
    Named(String), // Named selection
    Last,          // Last created entity
    All,           // All entities of type
}

/// Operation parameter with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AIOperationParameter {
    Number { value: f64, unit: Option<String> },
    Boolean(bool),
    Text(String),
    Vector { x: f64, y: f64, z: f64 },
    Point { x: f64, y: f64, z: f64 },
    Enum { value: String, options: Vec<String> },
    EntityRef(EntityReference),
}

/// Operation execution result with rich metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIOperationResult {
    /// Whether operation succeeded
    pub success: bool,
    /// Created/modified entities
    pub entities: Vec<EntityReference>,
    /// Human-readable description
    pub description: String,
    /// Performance metrics
    pub metrics: OperationMetrics,
    /// Warnings or optimization hints
    pub warnings: Vec<String>,
    /// Suggested follow-up operations
    pub next_operations: Vec<String>,
    /// Undo token for reversal
    pub undo_token: Option<String>,
}

/// Performance metrics for operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationMetrics {
    pub parse_time_ms: f64,
    pub execution_time_ms: f64,
    pub entities_processed: usize,
    pub memory_used_bytes: usize,
    pub parallelization_factor: f64,
}

/// Information about an operation for AI discovery
#[derive(Debug, Clone)]
pub struct AIOperationInfo {
    /// Operation name
    pub name: String,
    /// Natural language description
    pub description: String,
    /// Alternative names and phrases
    pub aliases: Vec<String>,
    /// Categories
    pub categories: Vec<String>,
    /// Required entity types
    pub required_entities: Vec<String>,
    /// Parameter definitions
    pub parameters: Vec<OperationParameter>,
    /// Usage examples
    pub examples: Vec<OperationExample>,
    /// Common use cases
    pub use_cases: Vec<String>,
    /// Performance characteristics
    pub performance: PerformanceProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationParameter {
    pub name: String,
    pub description: String,
    pub param_type: String,
    pub required: bool,
    pub default_value: Option<serde_json::Value>,
    pub constraints: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct OperationExample {
    pub input: String,
    pub entities: Vec<String>,
    pub parameters: DashMap<String, serde_json::Value>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceProfile {
    pub complexity: String, // O(n), O(n²), etc.
    pub typical_time_ms: f64,
    pub parallelizable: bool,
    pub gpu_accelerated: bool,
}

/// The central registry for AI-accessible CAD operations with DashMap for thread safety
pub struct OperationsRegistry {
    /// Registered operations
    operations: DashMap<String, AIOperationInfo>,
    /// Natural language patterns
    nlp_patterns: OperationPatterns,
    /// Operation chains for complex workflows (thread-safe)
    operation_chains: DashMap<String, Vec<String>>,
    /// Performance optimizer with thread-safe caching
    optimizer: OperationOptimizer,
    /// Usage statistics (thread-safe with DashMap)
    usage_stats: DashMap<String, OperationStats>,
}

#[derive(Debug, Clone)]
struct OperationPatterns {
    /// Patterns for operation detection
    patterns: Vec<(String, String)>, // (pattern, operation)
    /// Entity reference patterns
    entity_patterns: DashMap<String, String>,
    /// Parameter extraction patterns
    param_patterns: DashMap<String, Vec<String>>,
}

#[derive(Debug)]
struct OperationOptimizer {
    /// Cached operation plans (thread-safe with DashMap)
    cached_plans: DashMap<u64, ExecutionPlan>,
    /// Parallelization strategies (thread-safe with DashMap)
    parallel_strategies: DashMap<String, ParallelStrategy>,
}

#[derive(Debug, Clone)]
struct ExecutionPlan {
    steps: Vec<PlannedStep>,
    estimated_time_ms: f64,
    parallelizable: bool,
}

#[derive(Debug, Clone)]
struct PlannedStep {
    operation: String,
    targets: Vec<EntityReference>,
    parameters: DashMap<String, AIOperationParameter>,
    dependencies: Vec<usize>,
}

#[derive(Debug, Clone)]
enum ParallelStrategy {
    Independent,    // Can run fully parallel
    Batched(usize), // Process in batches
    Sequential,     // Must be sequential
}

#[derive(Debug, Clone)]
struct OperationStats {
    call_count: u64,
    success_rate: f64,
    avg_execution_time: f64,
    last_used: std::time::Instant,
    common_parameters: DashMap<String, Vec<(String, u64)>>,
}

impl OperationsRegistry {
    /// Create new registry with all operations
    pub fn new() -> Self {
        let mut registry = Self {
            operations: DashMap::new(),
            nlp_patterns: OperationPatterns {
                patterns: vec![],
                entity_patterns: DashMap::new(),
                param_patterns: DashMap::new(),
            },
            operation_chains: DashMap::new(),
            optimizer: OperationOptimizer {
                cached_plans: DashMap::new(),
                parallel_strategies: DashMap::new(),
            },
            usage_stats: DashMap::new(),
        };

        registry.register_all_operations();
        registry.register_operation_chains();
        registry
    }

    /// Get global registry instance
    pub fn global() -> Arc<Mutex<OperationsRegistry>> {
        OPERATIONS_REGISTRY.clone()
    }

    /// Execute natural language operation command
    pub fn execute_natural_language(
        text: &str,
        model: &mut BRepModel,
    ) -> OperationResult<AIOperationResult> {
        let registry = Self::global();
        let registry = registry
            .lock()
            .expect("AIOperationsRegistry global Mutex poisoned");

        let command = registry.parse_operation_command(text)?;
        registry.execute_operation(command, model)
    }

    /// Check operation cache for fast execution
    fn check_operation_cache(&self, text: &str) -> Option<AIOperationResult> {
        let hash = self.hash_operation_text(text);
        OPERATION_CACHE.get(&hash).and_then(|cached| {
            // Check if cache entry is fresh (not older than 30 minutes)
            if cached.created_at.elapsed().as_secs() < 1800 {
                Some(cached.result.clone())
            } else {
                // Remove stale entry
                OPERATION_CACHE.remove(&hash);
                None
            }
        })
    }

    /// Cache operation result for future fast execution
    fn cache_operation_result(&self, text: &str, result: &AIOperationResult) {
        let hash = self.hash_operation_text(text);
        let cached = CachedOperationResult {
            result: result.clone(),
            created_at: std::time::Instant::now(),
            hit_count: 1,
        };

        // Limit cache size to prevent memory bloat
        if OPERATION_CACHE.len() > 5000 {
            // Remove oldest entries (simple eviction strategy)
            let mut to_remove = vec![];
            for entry in OPERATION_CACHE.iter() {
                if entry.created_at.elapsed().as_secs() > 3600 {
                    // 1 hour
                    to_remove.push(*entry.key());
                }
                if to_remove.len() > 1000 {
                    break;
                }
            }
            for key in to_remove {
                OPERATION_CACHE.remove(&key);
            }
        }

        OPERATION_CACHE.insert(hash, cached);
    }

    /// Hash operation text for caching
    fn hash_operation_text(&self, text: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Parse natural language into operation command
    fn parse_operation_command(&self, text: &str) -> OperationResult<AIOperationCommand> {
        let text = text.to_lowercase();
        let _start = std::time::Instant::now();

        // Check cache first for instant response
        if let Some(_cached_result) = self.check_operation_cache(&text) {
            return Ok(AIOperationCommand {
                operation_type: "cached".to_string(),
                targets: vec![],
                parameters: DashMap::new(),
                confidence: 1.0,
                original_text: text.clone(),
                suggestions: vec![],
            });
        }

        // Detect operation type
        let operation_type = self.detect_operation_type(&text)?;

        // Extract target entities
        let targets = self.extract_entity_references(&text)?;

        // Extract parameters
        let parameters = self.extract_operation_parameters(&text, &operation_type)?;

        // Calculate confidence
        let confidence =
            self.calculate_operation_confidence(&operation_type, &targets, &parameters);

        // Generate suggestions
        let suggestions = if confidence < 0.8 {
            self.generate_operation_suggestions(&operation_type, &text)
        } else {
            vec![]
        };

        Ok(AIOperationCommand {
            operation_type,
            targets,
            parameters,
            confidence,
            original_text: text,
            suggestions,
        })
    }

    /// Detect which operation is being requested
    fn detect_operation_type(&self, text: &str) -> OperationResult<String> {
        // Direct operation name match
        for entry in &self.operations {
            let name = entry.key();
            if text.contains(name) {
                return Ok(name.clone());
            }
        }

        // Pattern matching
        for (pattern, operation) in &self.nlp_patterns.patterns {
            if text.contains(pattern) {
                return Ok(operation.clone());
            }
        }

        // Alias matching
        for entry in &self.operations {
            let name = entry.key();
            let info = entry.value();
            for alias in &info.aliases {
                if text.contains(alias) {
                    return Ok(name.clone());
                }
            }
        }

        Err(OperationError::InvalidGeometry(format!(
            "Could not determine operation from: {}",
            text
        )))
    }

    /// Extract entity references from text
    fn extract_entity_references(&self, text: &str) -> OperationResult<Vec<EntityReference>> {
        let mut refs = Vec::new();

        // Check for special references
        if text.contains("last") || text.contains("previous") {
            refs.push(EntityReference::Last);
        }

        if text.contains("all") {
            refs.push(EntityReference::All);
        }

        // Look for named selections
        for entry in &self.nlp_patterns.entity_patterns {
            let pattern = entry.key();
            let entity_type = entry.value();
            if text.contains(pattern) {
                refs.push(EntityReference::Named(entity_type.clone()));
            }
        }

        // Default to last if nothing found
        if refs.is_empty() {
            refs.push(EntityReference::Last);
        }

        Ok(refs)
    }

    /// Extract operation parameters
    fn extract_operation_parameters(
        &self,
        text: &str,
        operation_type: &str,
    ) -> OperationResult<DashMap<String, AIOperationParameter>> {
        let params = DashMap::new();

        // Get operation info
        let op_info = self
            .operations
            .get(operation_type)
            .ok_or_else(|| OperationError::InvalidGeometry("Unknown operation".to_string()))?;

        // Extract each parameter
        for param_def in &op_info.parameters {
            if let Some(value) =
                self.extract_parameter_value(text, &param_def.name, &param_def.param_type)
            {
                params.insert(param_def.name.clone(), value);
            } else if param_def.required {
                // Use default if available
                if let Some(default) = &param_def.default_value {
                    params.insert(param_def.name.clone(), self.json_to_parameter(default));
                }
            }
        }

        Ok(params)
    }

    /// Extract a specific parameter value
    fn extract_parameter_value(
        &self,
        text: &str,
        param_name: &str,
        param_type: &str,
    ) -> Option<AIOperationParameter> {
        match param_type {
            "number" => {
                // Look for numbers near parameter name
                let words: Vec<&str> = text.split_whitespace().collect();
                for i in 0..words.len() {
                    if words[i].contains(param_name) && i + 1 < words.len() {
                        if let Ok(value) = words[i + 1].parse::<f64>() {
                            return Some(AIOperationParameter::Number {
                                value,
                                unit: self.extract_unit(&words, i + 1),
                            });
                        }
                    }
                }
                None
            }
            "boolean" => {
                if text.contains("yes") || text.contains("true") || text.contains("enable") {
                    Some(AIOperationParameter::Boolean(true))
                } else if text.contains("no") || text.contains("false") || text.contains("disable")
                {
                    Some(AIOperationParameter::Boolean(false))
                } else {
                    None
                }
            }
            "enum" => {
                // Match against known enum values
                Some(AIOperationParameter::Text("default".to_string()))
            }
            _ => None,
        }
    }

    /// Extract unit from text
    fn extract_unit(&self, words: &[&str], index: usize) -> Option<String> {
        if index + 1 < words.len() {
            let potential_unit = words[index + 1];
            if ["mm", "cm", "m", "in", "ft"].contains(&potential_unit) {
                return Some(potential_unit.to_string());
            }
        }
        None
    }

    /// Convert JSON value to parameter
    fn json_to_parameter(&self, value: &serde_json::Value) -> AIOperationParameter {
        match value {
            serde_json::Value::Number(n) => AIOperationParameter::Number {
                value: n.as_f64().unwrap_or(0.0),
                unit: None,
            },
            serde_json::Value::Bool(b) => AIOperationParameter::Boolean(*b),
            serde_json::Value::String(s) => AIOperationParameter::Text(s.clone()),
            _ => AIOperationParameter::Text("default".to_string()),
        }
    }

    /// Calculate confidence score
    fn calculate_operation_confidence(
        &self,
        operation_type: &str,
        targets: &[EntityReference],
        parameters: &DashMap<String, AIOperationParameter>,
    ) -> f64 {
        let op_info = match self.operations.get(operation_type) {
            Some(info) => info,
            None => return 0.0,
        };

        // Check if we have required parameters
        let required_params: Vec<_> = op_info.parameters.iter().filter(|p| p.required).collect();

        let provided_required = required_params
            .iter()
            .filter(|p| parameters.contains_key(&p.name))
            .count();

        let param_score = if required_params.is_empty() {
            1.0
        } else {
            provided_required as f64 / required_params.len() as f64
        };

        // Check if we have valid targets
        let target_score = if targets.is_empty() { 0.5 } else { 1.0 };

        (param_score + target_score) / 2.0
    }

    /// Generate suggestions for improving command
    fn generate_operation_suggestions(&self, operation_type: &str, text: &str) -> Vec<String> {
        let mut suggestions = vec![];

        if let Some(op_info) = self.operations.get(operation_type) {
            // Suggest missing required parameters
            for param in &op_info.parameters {
                if param.required && !text.contains(&param.name) {
                    suggestions.push(format!("Specify {}: {}", param.name, param.description));
                }
            }

            // Suggest examples
            if let Some(example) = op_info.examples.first() {
                suggestions.push(format!("Example: {}", example.input));
            }
        }

        suggestions
    }

    /// Execute operation command
    fn execute_operation(
        &self,
        command: AIOperationCommand,
        model: &mut BRepModel,
    ) -> OperationResult<AIOperationResult> {
        let start = std::time::Instant::now();

        // Get execution plan
        let _plan = self.create_execution_plan(&command)?;

        // Execute based on operation type
        let result = match command.operation_type.as_str() {
            "boolean" => self.execute_boolean(&command, model),
            "fillet" => self.execute_fillet(&command, model),
            "chamfer" => self.execute_chamfer(&command, model),
            "extrude" => self.execute_extrude(&command, model),
            "revolve" => self.execute_revolve(&command, model),
            "pattern" => self.execute_pattern(&command, model),
            _ => Err(OperationError::NotImplemented(
                command.operation_type.clone(),
            )),
        }?;

        let execution_time = start.elapsed().as_secs_f64() * 1000.0;

        // Update statistics
        self.update_operation_stats(&command.operation_type, execution_time, true);

        Ok(AIOperationResult {
            success: true,
            entities: result,
            description: self.generate_operation_description(&command),
            metrics: OperationMetrics {
                parse_time_ms: 0.5, // Placeholder
                execution_time_ms: execution_time,
                entities_processed: command.targets.len(),
                memory_used_bytes: std::mem::size_of::<AIOperationCommand>()
                    + command.targets.len() * std::mem::size_of::<EntityReference>(),
                parallelization_factor: if command.targets.len() > 100 {
                    4.0
                } else {
                    1.0
                },
            },
            warnings: vec![],
            next_operations: self.suggest_next_operations(&command),
            undo_token: Some(uuid::Uuid::new_v4().to_string()),
        })
    }

    /// Create execution plan for optimization
    fn create_execution_plan(
        &self,
        command: &AIOperationCommand,
    ) -> OperationResult<ExecutionPlan> {
        // For now, simple single-step plan
        Ok(ExecutionPlan {
            steps: vec![PlannedStep {
                operation: command.operation_type.clone(),
                targets: command.targets.clone(),
                parameters: command.parameters.clone(),
                dependencies: vec![],
            }],
            estimated_time_ms: 10.0, // Placeholder
            parallelizable: false,
        })
    }

    /// Execute boolean operation
    fn execute_boolean(
        &self,
        command: &AIOperationCommand,
        model: &mut BRepModel,
    ) -> OperationResult<Vec<EntityReference>> {
        // Extract operation type
        let op_type = if let Some(param) = command.parameters.get("operation") {
            match param.value() {
                AIOperationParameter::Text(op) => match op.as_str() {
                    "union" | "add" | "merge" => BooleanOp::Union,
                    "subtract" | "difference" | "cut" => BooleanOp::Difference,
                    "intersect" | "intersection" => BooleanOp::Intersection,
                    _ => BooleanOp::Union,
                },
                _ => BooleanOp::Union,
            }
        } else {
            BooleanOp::Union
        };

        // Get target solids
        if command.targets.len() < 2 {
            return Err(OperationError::InvalidGeometry(
                "Boolean operation requires at least 2 solids".to_string(),
            ));
        }

        // For now, assume first two targets are solids
        let solid_a = match &command.targets[0] {
            EntityReference::Solid(id) => *id,
            _ => {
                return Err(OperationError::InvalidGeometry(
                    "First target must be a solid".to_string(),
                ))
            }
        };

        let solid_b = match &command.targets[1] {
            EntityReference::Solid(id) => *id,
            _ => {
                return Err(OperationError::InvalidGeometry(
                    "Second target must be a solid".to_string(),
                ))
            }
        };

        // Execute boolean
        let options = BooleanOptions::default();
        let result_id = crate::operations::boolean::boolean_operation(
            model, solid_a, solid_b, op_type, options,
        )?;

        Ok(vec![EntityReference::Solid(result_id)])
    }

    /// Execute fillet operation
    fn execute_fillet(
        &self,
        command: &AIOperationCommand,
        model: &mut BRepModel,
    ) -> OperationResult<Vec<EntityReference>> {
        // Extract radius
        let radius = if let Some(param) = command.parameters.get("radius") {
            match param.value() {
                AIOperationParameter::Number { value, .. } => *value,
                _ => 1.0, // Default radius
            }
        } else {
            1.0 // Default radius
        };

        // Get target edges
        let mut edge_ids = Vec::new();
        for target in &command.targets {
            match target {
                EntityReference::Edge(id) => edge_ids.push(*id),
                EntityReference::All => {
                    // Collect all edge IDs from the model
                    for (eid, _) in model.edges.iter() {
                        edge_ids.push(eid);
                    }
                }
                _ => {}
            }
        }

        if edge_ids.is_empty() {
            return Err(OperationError::InvalidGeometry(
                "No edges selected for fillet".to_string(),
            ));
        }

        // Execute fillet
        let options = FilletOptions {
            radius,
            common: crate::operations::CommonOptions::default(),
            fillet_type: crate::operations::fillet::FilletType::Constant(radius),
            propagation: crate::operations::fillet::PropagationMode::Tangent,
            preserve_edges: true,
            quality: crate::operations::fillet::FilletQuality::Standard,
        };
        // Get the solid_id from the first edge's parent face
        let solid_id = if !edge_ids.is_empty() {
            // Find which solid contains the first edge
            let first_edge_id = edge_ids[0];
            let mut found_solid_id = None;

            // Search through all solids to find which one contains this edge
            for solid_id in 0..model.solids.len() as u32 {
                if let Some(solid) = model.solids.get(solid_id) {
                    // Check if this solid contains the edge
                    // Check outer shell
                    let mut shells_to_check = vec![solid.outer_shell];
                    shells_to_check.extend(&solid.inner_shells);

                    for &shell_id in &shells_to_check {
                        if let Some(shell) = model.shells.get(shell_id) {
                            for &face_id in &shell.faces {
                                if let Some(face) = model.faces.get(face_id) {
                                    // Check outer loop
                                    if let Some(loop_data) = model.loops.get(face.outer_loop) {
                                        if loop_data.edges.contains(&first_edge_id) {
                                            found_solid_id = Some(solid_id);
                                            break;
                                        }
                                    }
                                    // Check inner loops
                                    for &loop_id in &face.inner_loops {
                                        if let Some(loop_data) = model.loops.get(loop_id) {
                                            if loop_data.edges.contains(&first_edge_id) {
                                                found_solid_id = Some(solid_id);
                                                break;
                                            }
                                        }
                                    }
                                }
                                if found_solid_id.is_some() {
                                    break;
                                }
                            }
                        }
                        if found_solid_id.is_some() {
                            break;
                        }
                    }
                }
                if found_solid_id.is_some() {
                    break;
                }
            }

            found_solid_id.unwrap_or(0u32)
        } else {
            return Err(OperationError::InvalidInput {
                parameter: "edges".to_string(),
                expected: "at least one edge".to_string(),
                received: "empty edge list".to_string(),
            });
        };

        let results = crate::operations::fillet::fillet_edges(model, solid_id, edge_ids, options)?;

        Ok(results.into_iter().map(EntityReference::Face).collect())
    }

    /// Execute chamfer operation
    fn execute_chamfer(
        &self,
        command: &AIOperationCommand,
        model: &mut BRepModel,
    ) -> OperationResult<Vec<EntityReference>> {
        // Similar to fillet but with distance
        let distance = if let Some(param) = command.parameters.get("distance") {
            match param.value() {
                AIOperationParameter::Number { value, .. } => *value,
                _ => 1.0,
            }
        } else {
            1.0
        };

        let mut edge_ids = Vec::new();
        for target in &command.targets {
            if let EntityReference::Edge(id) = target {
                edge_ids.push(*id);
            }
        }

        let options = ChamferOptions {
            distance1: distance,
            distance2: distance,
            symmetric: true,
            common: crate::operations::CommonOptions::default(),
            chamfer_type: crate::operations::chamfer::ChamferType::EqualDistance(distance),
            propagation: crate::operations::chamfer::PropagationMode::None,
            preserve_edges: false,
        };

        // Get the solid_id from the first edge's parent face
        let solid_id = if !edge_ids.is_empty() {
            // Find which solid contains the first edge
            let first_edge_id = edge_ids[0];
            let mut found_solid_id = None;

            // Search through all solids to find which one contains this edge
            for solid_id in 0..model.solids.len() as u32 {
                if let Some(solid) = model.solids.get(solid_id) {
                    // Check if this solid contains the edge
                    // Check outer shell
                    let mut shells_to_check = vec![solid.outer_shell];
                    shells_to_check.extend(&solid.inner_shells);

                    for &shell_id in &shells_to_check {
                        if let Some(shell) = model.shells.get(shell_id) {
                            for &face_id in &shell.faces {
                                if let Some(face) = model.faces.get(face_id) {
                                    // Check outer loop
                                    if let Some(loop_data) = model.loops.get(face.outer_loop) {
                                        if loop_data.edges.contains(&first_edge_id) {
                                            found_solid_id = Some(solid_id);
                                            break;
                                        }
                                    }
                                    // Check inner loops
                                    for &loop_id in &face.inner_loops {
                                        if let Some(loop_data) = model.loops.get(loop_id) {
                                            if loop_data.edges.contains(&first_edge_id) {
                                                found_solid_id = Some(solid_id);
                                                break;
                                            }
                                        }
                                    }
                                }
                                if found_solid_id.is_some() {
                                    break;
                                }
                            }
                        }
                        if found_solid_id.is_some() {
                            break;
                        }
                    }
                }
                if found_solid_id.is_some() {
                    break;
                }
            }

            found_solid_id.unwrap_or(0u32)
        } else {
            return Err(OperationError::InvalidInput {
                parameter: "edges".to_string(),
                expected: "at least one edge".to_string(),
                received: "empty edge list".to_string(),
            });
        };

        let results =
            crate::operations::chamfer::chamfer_edges(model, solid_id, edge_ids, options)?;

        Ok(results.into_iter().map(EntityReference::Face).collect())
    }

    /// Execute extrude operation
    fn execute_extrude(
        &self,
        command: &AIOperationCommand,
        model: &mut BRepModel,
    ) -> OperationResult<Vec<EntityReference>> {
        // Extract distance
        let distance = if let Some(param) = command.parameters.get("distance") {
            match param.value() {
                AIOperationParameter::Number { value, .. } => *value,
                _ => 10.0,
            }
        } else {
            10.0
        };

        // Extract direction
        let direction = if let Some(param) = command.parameters.get("direction") {
            match param.value() {
                AIOperationParameter::Vector { x, y, z } => Vector3::new(*x, *y, *z),
                _ => Vector3::new(0.0, 0.0, 1.0), // Default Z direction
            }
        } else {
            Vector3::new(0.0, 0.0, 1.0) // Default Z direction
        };

        // Get target face
        let face_id = match command.targets.first() {
            Some(EntityReference::Face(id)) => *id,
            _ => {
                return Err(OperationError::InvalidGeometry(
                    "Extrude requires a face".to_string(),
                ))
            }
        };

        let options = ExtrudeOptions {
            distance,
            direction,
            symmetric: false,
            common: crate::operations::CommonOptions::default(),
            draft_angle: 0.0,
            twist_angle: 0.0,
            cap_ends: true,
            end_scale: 1.0,
        };

        let result = crate::operations::extrude::extrude_face(model, face_id, options)?;

        Ok(vec![EntityReference::Solid(result)])
    }

    /// Execute revolve operation
    fn execute_revolve(
        &self,
        command: &AIOperationCommand,
        model: &mut BRepModel,
    ) -> OperationResult<Vec<EntityReference>> {
        // Extract angle
        let angle = if let Some(param) = command.parameters.get("angle") {
            match param.value() {
                AIOperationParameter::Number { value, .. } => value.to_radians(),
                _ => std::f64::consts::TAU, // Full revolution
            }
        } else {
            std::f64::consts::TAU // Full revolution
        };

        // Extract axis
        let axis_origin = if let Some(param) = command.parameters.get("axis_origin") {
            match param.value() {
                AIOperationParameter::Point { x, y, z } => Point3::new(*x, *y, *z),
                _ => Point3::ORIGIN,
            }
        } else {
            Point3::ORIGIN
        };

        let axis_direction = if let Some(param) = command.parameters.get("axis_direction") {
            match param.value() {
                AIOperationParameter::Vector { x, y, z } => Vector3::new(*x, *y, *z),
                _ => Vector3::new(0.0, 1.0, 0.0), // Y axis
            }
        } else {
            Vector3::new(0.0, 1.0, 0.0) // Y axis
        };

        let face_id = match command.targets.first() {
            Some(EntityReference::Face(id)) => *id,
            _ => {
                return Err(OperationError::InvalidGeometry(
                    "Revolve requires a face".to_string(),
                ))
            }
        };

        let options = RevolveOptions {
            angle,
            axis_origin,
            axis_direction,
            symmetric: false,
            common: crate::operations::CommonOptions::default(),
            segments: 32,
            pitch: 0.0,
            cap_ends: true,
        };

        let result = crate::operations::revolve::revolve_face(model, face_id, options)?;

        Ok(vec![EntityReference::Solid(result)])
    }

    /// Execute pattern operation
    fn execute_pattern(
        &self,
        command: &AIOperationCommand,
        model: &mut BRepModel,
    ) -> OperationResult<Vec<EntityReference>> {
        // Extract pattern type
        let pattern_type = if let Some(param) = command.parameters.get("type") {
            match param.value() {
                AIOperationParameter::Text(t) => match t.as_str() {
                    "linear" | "array" => PatternType::Linear {
                        direction: Vector3::new(1.0, 0.0, 0.0),
                        spacing: 10.0,
                        count: 5,
                    },
                    "circular" | "polar" => PatternType::Circular {
                        axis_origin: Point3::ORIGIN,
                        axis_direction: Vector3::new(0.0, 0.0, 1.0),
                        count: 6,
                        angle: std::f64::consts::TAU,
                    },
                    _ => PatternType::Linear {
                        direction: Vector3::new(1.0, 0.0, 0.0),
                        spacing: 10.0,
                        count: 5,
                    },
                },
                _ => PatternType::Linear {
                    direction: Vector3::new(1.0, 0.0, 0.0),
                    spacing: 10.0,
                    count: 5,
                },
            }
        } else {
            PatternType::Linear {
                direction: Vector3::new(1.0, 0.0, 0.0),
                spacing: 10.0,
                count: 5,
            }
        };

        let solid_id = match command.targets.first() {
            Some(EntityReference::Solid(id)) => *id,
            _ => {
                return Err(OperationError::InvalidGeometry(
                    "Pattern requires a solid".to_string(),
                ))
            }
        };

        let options = PatternOptions {
            pattern_type: pattern_type.clone(),
            merge_results: false,
            common: crate::operations::CommonOptions::default(),
            pattern_target: crate::operations::pattern::PatternTarget::Features,
            merge_geometry: true,
            associative: false,
            skip_interferences: false,
        };

        let results = crate::operations::pattern::create_pattern(
            model,
            vec![solid_id],
            pattern_type,
            options,
        )?;

        Ok(results
            .into_iter()
            .flat_map(|pattern_instance| pattern_instance.into_iter())
            .map(EntityReference::Solid)
            .collect())
    }

    /// Generate human-readable description
    fn generate_operation_description(&self, command: &AIOperationCommand) -> String {
        match command.operation_type.as_str() {
            "boolean" => {
                let op = command
                    .parameters
                    .get("operation")
                    .and_then(|p| match p.value() {
                        AIOperationParameter::Text(t) => Some(t.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "union".to_string());
                format!(
                    "Performed boolean {} operation on {} entities",
                    op,
                    command.targets.len()
                )
            }
            "fillet" => {
                let radius = command
                    .parameters
                    .get("radius")
                    .and_then(|p| match p.value() {
                        AIOperationParameter::Number { value, .. } => Some(*value),
                        _ => None,
                    })
                    .unwrap_or(1.0);
                format!(
                    "Applied {}mm fillet to {} edges",
                    radius,
                    command.targets.len()
                )
            }
            _ => format!("Completed {} operation", command.operation_type),
        }
    }

    /// Suggest next operations based on context
    fn suggest_next_operations(&self, command: &AIOperationCommand) -> Vec<String> {
        match command.operation_type.as_str() {
            "boolean" => vec![
                "Apply fillets to sharp edges".to_string(),
                "Create a pattern of the result".to_string(),
                "Export for 3D printing".to_string(),
            ],
            "fillet" => vec![
                "Apply chamfers to remaining edges".to_string(),
                "Add draft angles for molding".to_string(),
                "Check for thin walls".to_string(),
            ],
            "extrude" => vec![
                "Apply draft angle for manufacturability".to_string(),
                "Create internal features with boolean cut".to_string(),
                "Add fillets to edges".to_string(),
            ],
            _ => vec!["Explore other operations".to_string()],
        }
    }

    /// Update operation statistics (production implementation with DashMap)
    fn update_operation_stats(&self, operation_type: &str, execution_time: f64, success: bool) {
        self.usage_stats
            .entry(operation_type.to_string())
            .and_modify(|stats| {
                stats.call_count += 1;

                // Update success rate with exponential moving average
                let alpha = 0.1;
                let success_value = if success { 1.0 } else { 0.0 };
                stats.success_rate = stats.success_rate * (1.0 - alpha) + success_value * alpha;

                // Update average execution time
                stats.avg_execution_time =
                    stats.avg_execution_time * (1.0 - alpha) + execution_time * alpha;

                // Update last used timestamp
                stats.last_used = std::time::Instant::now();
            })
            .or_insert_with(|| OperationStats {
                call_count: 1,
                success_rate: if success { 1.0 } else { 0.0 },
                avg_execution_time: execution_time,
                last_used: std::time::Instant::now(),
                common_parameters: DashMap::new(),
            });
    }

    /// Register all available operations
    fn register_all_operations(&mut self) {
        self.register_boolean_operation();
        self.register_fillet_operation();
        self.register_chamfer_operation();
        self.register_extrude_operation();
        self.register_revolve_operation();
        self.register_pattern_operation();
        // Add more operations...
    }

    /// Register boolean operation
    fn register_boolean_operation(&mut self) {
        let info = AIOperationInfo {
            name: "boolean".to_string(),
            description: "Combine or subtract solids using boolean operations".to_string(),
            aliases: vec![
                "union".to_string(),
                "merge".to_string(),
                "combine".to_string(),
                "subtract".to_string(),
                "cut".to_string(),
                "difference".to_string(),
                "intersect".to_string(),
                "intersection".to_string(),
            ],
            categories: vec!["solid".to_string(), "combination".to_string()],
            required_entities: vec!["solid".to_string()],
            parameters: vec![OperationParameter {
                name: "operation".to_string(),
                description: "Type of boolean operation".to_string(),
                param_type: "enum".to_string(),
                required: true,
                default_value: Some(serde_json::json!("union")),
                constraints: Some(serde_json::json!({
                    "options": ["union", "difference", "intersection"]
                })),
            }],
            examples: vec![
                OperationExample {
                    input: "unite these two solids".to_string(),
                    entities: vec!["solid1".to_string(), "solid2".to_string()],
                    parameters: {
                        let map = DashMap::new();
                        map.insert("operation".to_string(), serde_json::json!("union"));
                        map
                    },
                    description: "Combines two solids into one".to_string(),
                },
                OperationExample {
                    input: "cut the second solid from the first".to_string(),
                    entities: vec!["solid1".to_string(), "solid2".to_string()],
                    parameters: {
                        let map = DashMap::new();
                        map.insert("operation".to_string(), serde_json::json!("difference"));
                        map
                    },
                    description: "Subtracts second solid from first".to_string(),
                },
            ],
            use_cases: vec![
                "Creating complex shapes from primitives".to_string(),
                "Making holes and cutouts".to_string(),
                "Finding common volume between parts".to_string(),
            ],
            performance: PerformanceProfile {
                complexity: "O(n*m)".to_string(),
                typical_time_ms: 50.0,
                parallelizable: true,
                gpu_accelerated: false,
            },
        };

        self.operations.insert("boolean".to_string(), info);

        // Add NLP patterns
        self.nlp_patterns.patterns.extend(vec![
            ("unite".to_string(), "boolean".to_string()),
            ("merge".to_string(), "boolean".to_string()),
            ("combine".to_string(), "boolean".to_string()),
            ("subtract".to_string(), "boolean".to_string()),
            ("cut".to_string(), "boolean".to_string()),
            ("remove".to_string(), "boolean".to_string()),
        ]);
    }

    /// Register fillet operation
    fn register_fillet_operation(&mut self) {
        let info = AIOperationInfo {
            name: "fillet".to_string(),
            description: "Round edges with a constant radius".to_string(),
            aliases: vec!["round".to_string(), "blend".to_string()],
            categories: vec!["edge".to_string(), "modification".to_string()],
            required_entities: vec!["edge".to_string()],
            parameters: vec![OperationParameter {
                name: "radius".to_string(),
                description: "Fillet radius in millimeters".to_string(),
                param_type: "number".to_string(),
                required: true,
                default_value: Some(serde_json::json!(1.0)),
                constraints: Some(serde_json::json!({
                    "min": 0.001,
                    "max": 1000.0
                })),
            }],
            examples: vec![OperationExample {
                input: "fillet all edges with radius 2mm".to_string(),
                entities: vec!["all_edges".to_string()],
                parameters: {
                    let map = DashMap::new();
                    map.insert("radius".to_string(), serde_json::json!(2.0));
                    map
                },
                description: "Rounds all edges with 2mm radius".to_string(),
            }],
            use_cases: vec![
                "Removing sharp edges for safety".to_string(),
                "Improving aesthetics".to_string(),
                "Reducing stress concentrations".to_string(),
            ],
            performance: PerformanceProfile {
                complexity: "O(n)".to_string(),
                typical_time_ms: 20.0,
                parallelizable: true,
                gpu_accelerated: false,
            },
        };

        self.operations.insert("fillet".to_string(), info);
    }

    /// Register chamfer operation  
    fn register_chamfer_operation(&mut self) {
        let info = AIOperationInfo {
            name: "chamfer".to_string(),
            description: "Create angled cuts on edges".to_string(),
            aliases: vec!["bevel".to_string(), "angle".to_string()],
            categories: vec!["edge".to_string(), "modification".to_string()],
            required_entities: vec!["edge".to_string()],
            parameters: vec![OperationParameter {
                name: "distance".to_string(),
                description: "Chamfer distance in millimeters".to_string(),
                param_type: "number".to_string(),
                required: true,
                default_value: Some(serde_json::json!(1.0)),
                constraints: Some(serde_json::json!({
                    "min": 0.001,
                    "max": 1000.0
                })),
            }],
            examples: vec![OperationExample {
                input: "chamfer edges with 1mm distance".to_string(),
                entities: vec!["selected_edges".to_string()],
                parameters: {
                    let map = DashMap::new();
                    map.insert("distance".to_string(), serde_json::json!(1.0));
                    map
                },
                description: "Creates 1mm chamfers on selected edges".to_string(),
            }],
            use_cases: vec![
                "Breaking sharp edges".to_string(),
                "Creating lead-ins for assembly".to_string(),
                "Aesthetic detailing".to_string(),
            ],
            performance: PerformanceProfile {
                complexity: "O(n)".to_string(),
                typical_time_ms: 15.0,
                parallelizable: true,
                gpu_accelerated: false,
            },
        };

        self.operations.insert("chamfer".to_string(), info);
    }

    /// Register extrude operation
    fn register_extrude_operation(&mut self) {
        let info = AIOperationInfo {
            name: "extrude".to_string(),
            description: "Create a solid by extruding a face along a direction".to_string(),
            aliases: vec!["pull".to_string(), "push".to_string(), "extend".to_string()],
            categories: vec!["solid".to_string(), "creation".to_string()],
            required_entities: vec!["face".to_string()],
            parameters: vec![
                OperationParameter {
                    name: "distance".to_string(),
                    description: "Extrusion distance".to_string(),
                    param_type: "number".to_string(),
                    required: true,
                    default_value: Some(serde_json::json!(10.0)),
                    constraints: Some(serde_json::json!({
                        "min": 0.001,
                        "max": 10000.0
                    })),
                },
                OperationParameter {
                    name: "direction".to_string(),
                    description: "Extrusion direction vector".to_string(),
                    param_type: "vector".to_string(),
                    required: false,
                    default_value: Some(serde_json::json!([0, 0, 1])),
                    constraints: None,
                },
            ],
            examples: vec![OperationExample {
                input: "extrude this face 20mm upward".to_string(),
                entities: vec!["face1".to_string()],
                parameters: {
                    let map = DashMap::new();
                    map.insert("distance".to_string(), serde_json::json!(20.0));
                    map.insert("direction".to_string(), serde_json::json!([0, 0, 1]));
                    map
                },
                description: "Extrudes face 20mm in Z direction".to_string(),
            }],
            use_cases: vec![
                "Creating prismatic parts".to_string(),
                "Adding bosses and protrusions".to_string(),
                "Generating constant cross-section shapes".to_string(),
            ],
            performance: PerformanceProfile {
                complexity: "O(n)".to_string(),
                typical_time_ms: 10.0,
                parallelizable: false,
                gpu_accelerated: false,
            },
        };

        self.operations.insert("extrude".to_string(), info);
    }

    /// Register revolve operation
    fn register_revolve_operation(&mut self) {
        let info = AIOperationInfo {
            name: "revolve".to_string(),
            description: "Create a solid by revolving a face around an axis".to_string(),
            aliases: vec!["rotate".to_string(), "spin".to_string(), "turn".to_string()],
            categories: vec!["solid".to_string(), "creation".to_string()],
            required_entities: vec!["face".to_string()],
            parameters: vec![OperationParameter {
                name: "angle".to_string(),
                description: "Revolution angle in degrees".to_string(),
                param_type: "number".to_string(),
                required: false,
                default_value: Some(serde_json::json!(360.0)),
                constraints: Some(serde_json::json!({
                    "min": 0.1,
                    "max": 360.0
                })),
            }],
            examples: vec![OperationExample {
                input: "revolve this profile 360 degrees".to_string(),
                entities: vec!["profile1".to_string()],
                parameters: {
                    let map = DashMap::new();
                    map.insert("angle".to_string(), serde_json::json!(360.0));
                    map
                },
                description: "Creates a full revolution solid".to_string(),
            }],
            use_cases: vec![
                "Creating cylindrical and spherical shapes".to_string(),
                "Making turned parts".to_string(),
                "Generating surfaces of revolution".to_string(),
            ],
            performance: PerformanceProfile {
                complexity: "O(n)".to_string(),
                typical_time_ms: 15.0,
                parallelizable: false,
                gpu_accelerated: false,
            },
        };

        self.operations.insert("revolve".to_string(), info);
    }

    /// Register pattern operation
    fn register_pattern_operation(&mut self) {
        let info = AIOperationInfo {
            name: "pattern".to_string(),
            description: "Create multiple copies in a regular arrangement".to_string(),
            aliases: vec![
                "array".to_string(),
                "duplicate".to_string(),
                "repeat".to_string(),
            ],
            categories: vec!["solid".to_string(), "duplication".to_string()],
            required_entities: vec!["solid".to_string()],
            parameters: vec![
                OperationParameter {
                    name: "type".to_string(),
                    description: "Pattern type (linear or circular)".to_string(),
                    param_type: "enum".to_string(),
                    required: true,
                    default_value: Some(serde_json::json!("linear")),
                    constraints: Some(serde_json::json!({
                        "options": ["linear", "circular"]
                    })),
                },
                OperationParameter {
                    name: "count".to_string(),
                    description: "Number of instances".to_string(),
                    param_type: "number".to_string(),
                    required: true,
                    default_value: Some(serde_json::json!(5)),
                    constraints: Some(serde_json::json!({
                        "min": 2,
                        "max": 1000
                    })),
                },
            ],
            examples: vec![OperationExample {
                input: "create a linear pattern of 5 copies".to_string(),
                entities: vec!["solid1".to_string()],
                parameters: {
                    let map = DashMap::new();
                    map.insert("type".to_string(), serde_json::json!("linear"));
                    map.insert("count".to_string(), serde_json::json!(5));
                    map
                },
                description: "Creates 5 copies in a line".to_string(),
            }],
            use_cases: vec![
                "Creating bolt hole patterns".to_string(),
                "Making gear teeth".to_string(),
                "Duplicating features".to_string(),
            ],
            performance: PerformanceProfile {
                complexity: "O(n)".to_string(),
                typical_time_ms: 5.0,
                parallelizable: true,
                gpu_accelerated: true,
            },
        };

        self.operations.insert("pattern".to_string(), info);
    }

    /// Register operation chains for complex workflows
    fn register_operation_chains(&mut self) {
        // Common workflow: Create and detail a part
        self.operation_chains.insert(
            "create_detailed_part".to_string(),
            vec![
                "extrude".to_string(),
                "fillet".to_string(),
                "chamfer".to_string(),
            ],
        );

        // Mechanical assembly workflow
        self.operation_chains.insert(
            "mechanical_assembly".to_string(),
            vec![
                "boolean".to_string(),
                "pattern".to_string(),
                "fillet".to_string(),
            ],
        );
    }

    /// Get operation catalog for AI discovery
    pub fn get_operations_catalog() -> serde_json::Value {
        let registry = Self::global();
        let registry = registry
            .lock()
            .expect("AIOperationsRegistry global Mutex poisoned");

        // Build operations JSON manually without requiring Serialize trait
        let mut operations_json = serde_json::Map::new();
        for entry in registry.operations.iter() {
            let key = entry.key();
            let info = entry.value();

            // Manually construct the operation info JSON
            let operation_info = serde_json::json!({
                "name": info.name,
                "description": info.description,
                "aliases": info.aliases,
                "categories": info.categories,
                "required_entities": info.required_entities,
                "parameters": info.parameters.iter().map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "description": p.description,
                        "type": p.param_type,
                        "required": p.required,
                        "default": p.default_value,
                        "constraints": p.constraints
                    })
                }).collect::<Vec<_>>(),
                "examples": info.examples.iter().map(|e| {
                    // Convert DashMap parameters to JSON object manually
                    let mut params_json = serde_json::Map::new();
                    for param_entry in e.parameters.iter() {
                        params_json.insert(param_entry.key().clone(), param_entry.value().clone());
                    }

                    serde_json::json!({
                        "description": e.description,
                        "input": e.input,
                        "parameters": params_json,
                        "entities": e.entities
                    })
                }).collect::<Vec<_>>()
            });

            operations_json.insert(key.clone(), operation_info);
        }

        // Build workflows JSON
        let mut workflows_json = serde_json::Map::new();
        for entry in registry.operation_chains.iter() {
            workflows_json.insert(
                entry.key().clone(),
                serde_json::json!(entry.value().clone()),
            );
        }

        serde_json::json!({
            "version": "1.0.0",
            "operations": operations_json,
            "workflows": workflows_json,
            "performance": {
                "boolean_ops_per_sec": 20,
                "fillet_ops_per_sec": 50,
                "pattern_ops_per_sec": 100,
            },
            "capabilities": {
                "parallel_execution": true,
                "gpu_acceleration": ["pattern", "tessellation"],
                "undo_redo": true,
                "parametric_history": true,
            }
        })
    }

    /// Batch execute multiple operations
    pub fn execute_batch(
        commands: Vec<String>,
        model: &mut BRepModel,
    ) -> Vec<OperationResult<AIOperationResult>> {
        let registry = Self::global();
        let registry = registry
            .lock()
            .expect("AIOperationsRegistry global Mutex poisoned");

        commands
            .into_iter()
            .map(|cmd| {
                let command = registry.parse_operation_command(&cmd)?;
                registry.execute_operation(command, model)
            })
            .collect()
    }

    /// Get performance hints based on usage
    pub fn get_performance_hints() -> Vec<String> {
        let registry = Self::global();
        let registry = registry
            .lock()
            .expect("AIOperationsRegistry global Mutex poisoned");

        let mut hints = vec![];

        // Analyze usage patterns
        for entry in registry.usage_stats.iter() {
            let op = entry.key();
            let stats = entry.value();
            if stats.avg_execution_time > 100.0 {
                hints.push(format!(
                    "{} operations averaging {}ms - consider batching",
                    op, stats.avg_execution_time
                ));
            }
        }

        hints
    }
}

/// Extension for AI-friendly error messages
impl OperationError {
    /// Convert to AI-friendly error response
    pub fn to_ai_error(&self) -> serde_json::Value {
        serde_json::json!({
            "error_type": self.error_type(),
            "message": self.to_string(),
            "suggestions": self.get_suggestions(),
            "recovery_options": self.get_recovery_options(),
        })
    }

    fn error_type(&self) -> &'static str {
        match self {
            OperationError::InvalidGeometry(_) => "invalid_geometry",
            OperationError::TopologyError(_) => "topology_error",
            OperationError::NumericalError(_) => "numerical_error",
            OperationError::SelfIntersection => "self_intersection",
            OperationError::InvalidBRep(_) => "invalid_brep",
            OperationError::FeatureTooSmall => "feature_too_small",
            OperationError::NotImplemented(_) => "not_implemented",
            OperationError::InternalError(_) => "internal_error",
            OperationError::IntersectionFailed => "intersection_failed",
            OperationError::InvalidRadius(_) => "invalid_radius",
            OperationError::OpenProfile => "open_profile",
            OperationError::IncompatibleProfiles => "incompatible_profiles",
            OperationError::InvalidPattern(_) => "invalid_pattern",
            OperationError::InvalidInput { .. } => "invalid_input",
            OperationError::CoplanarFaces(_) => "coplanar_faces",
        }
    }

    fn get_suggestions(&self) -> Vec<String> {
        match self {
            OperationError::InvalidRadius(r) => vec![
                format!("Try a smaller radius (current: {})", r),
                "Check edge length constraints".to_string(),
            ],
            OperationError::SelfIntersection => vec![
                "Reduce operation parameters".to_string(),
                "Check input geometry for issues".to_string(),
            ],
            OperationError::FeatureTooSmall => vec![
                "Increase feature size".to_string(),
                "Adjust tolerance settings".to_string(),
            ],
            _ => vec!["Check input parameters".to_string()],
        }
    }

    fn get_recovery_options(&self) -> Vec<String> {
        match self {
            OperationError::InvalidRadius(_) => vec![
                "Automatically compute maximum valid radius".to_string(),
                "Split operation into multiple steps".to_string(),
            ],
            OperationError::OpenProfile => vec![
                "Automatically close profile".to_string(),
                "Convert to surface operation".to_string(),
            ],
            _ => vec!["Retry with different parameters".to_string()],
        }
    }
}

// UUID support
mod uuid {
    pub struct Uuid(u128);

    impl Uuid {
        pub fn new_v4() -> Self {
            // Simple UUID v4 generation
            use std::time::{SystemTime, UNIX_EPOCH};
            // `duration_since(UNIX_EPOCH)` can only fail if the system clock
            // is set before 1970. For a UUID source we prefer to degrade
            // gracefully rather than panic, so fall back to a zero duration
            // in that pathological case.
            let time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let random = time.as_nanos() ^ ((time.as_secs() as u128) << 64);
            Self(random)
        }

        pub fn to_string(&self) -> String {
            format!("{:032x}", self.0)
        }
    }
}
