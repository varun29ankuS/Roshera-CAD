/// System context for AI awareness
///
/// This module provides comprehensive system state information for AI to understand
/// the current environment, users, workflow, and available commands.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Complete system context for AI awareness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContext {
    /// User session information
    pub session: SessionContext,

    /// Workflow state
    pub workflow: WorkflowContext,

    /// Available commands and their usage
    pub commands: CommandContext,

    /// System capabilities and features
    pub capabilities: SystemCapabilities,

    /// Current environment settings
    pub environment: EnvironmentContext,

    /// Real-time status
    pub status: SystemStatus,
}

/// Session and user information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    /// Current session ID
    pub session_id: String,

    /// Current user information
    pub current_user: UserInfo,

    /// List of all connected users
    pub connected_users: Vec<UserInfo>,

    /// Recently disconnected users
    pub recent_disconnections: Vec<UserDisconnection>,

    /// Session start time
    pub started_at: u64,

    /// Session metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// User information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    /// User ID
    pub id: String,

    /// Display name
    pub name: String,

    /// Role (owner, editor, viewer)
    pub role: String,

    /// Connection status
    pub status: UserStatus,

    /// Connected since
    pub connected_at: u64,

    /// Last activity
    pub last_active: u64,

    /// Current cursor position in 3D space
    pub cursor_position: Option<[f64; 3]>,

    /// Currently selected objects
    pub selected_objects: Vec<Uuid>,
}

/// User connection status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserStatus {
    Active,
    Idle,
    Away,
    Disconnected,
}

/// User disconnection info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDisconnection {
    pub user: UserInfo,
    pub disconnected_at: u64,
    pub reason: String,
}

/// Workflow context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowContext {
    /// Current workflow stage
    pub current_stage: WorkflowStage,

    /// Current step within the stage
    pub current_step: Option<String>,

    /// Available workflow stages
    pub available_stages: Vec<WorkflowStage>,

    /// Workflow history
    pub history: Vec<WorkflowHistoryEntry>,

    /// Current task or project name
    pub project_name: Option<String>,

    /// Active tool
    pub active_tool: Option<String>,

    /// Tool-specific parameters
    pub tool_parameters: HashMap<String, serde_json::Value>,
}

/// Workflow stages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkflowStage {
    Sketch,
    Part,
    Assembly,
    Drawing,
    Simulation,
    Manufacturing,
}

/// Workflow history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowHistoryEntry {
    pub stage: WorkflowStage,
    pub action: String,
    pub timestamp: u64,
    pub user_id: String,
}

/// Command context - all available commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandContext {
    /// Geometry creation commands
    pub creation_commands: Vec<CommandInfo>,

    /// Modification commands
    pub modification_commands: Vec<CommandInfo>,

    /// Query commands
    pub query_commands: Vec<CommandInfo>,

    /// Workflow commands
    pub workflow_commands: Vec<CommandInfo>,

    /// System commands
    pub system_commands: Vec<CommandInfo>,

    /// Command shortcuts/aliases
    pub aliases: HashMap<String, String>,
}

/// Command information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandInfo {
    /// Command name
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// Example usage
    pub examples: Vec<String>,

    /// Required parameters
    pub required_params: Vec<ParameterInfo>,

    /// Optional parameters
    pub optional_params: Vec<ParameterInfo>,

    /// Category
    pub category: String,

    /// Keyboard shortcut if any
    pub shortcut: Option<String>,
}

/// Parameter information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterInfo {
    pub name: String,
    pub param_type: String,
    pub description: String,
    pub default_value: Option<serde_json::Value>,
    pub constraints: Option<serde_json::Value>,
}

/// System capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemCapabilities {
    /// Supported geometry types
    pub geometry_types: Vec<String>,

    /// Available boolean operations
    pub boolean_operations: Vec<String>,

    /// Export formats
    pub export_formats: Vec<String>,

    /// Import formats
    pub import_formats: Vec<String>,

    /// Analysis capabilities
    pub analysis_features: Vec<String>,

    /// Collaboration features
    pub collaboration_features: Vec<String>,

    /// AI features
    pub ai_features: Vec<String>,

    /// Version control features
    pub version_control: bool,

    /// Real-time sync
    pub real_time_sync: bool,
}

/// Environment context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentContext {
    /// Unit system (metric/imperial)
    pub unit_system: String,

    /// Grid settings
    pub grid: GridInfo,

    /// Snap settings
    pub snap_settings: SnapSettings,

    /// Display settings
    pub display: DisplaySettings,

    /// Precision settings
    pub precision: PrecisionSettings,

    /// Theme (dark/light)
    pub theme: String,

    /// Language
    pub language: String,
}

/// Grid information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridInfo {
    pub enabled: bool,
    pub spacing: f64,
    pub major_lines: u32,
    pub visible: bool,
}

/// Snap settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapSettings {
    pub grid_snap: bool,
    pub object_snap: bool,
    pub angle_snap: bool,
    pub snap_tolerance: f64,
}

/// Display settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplaySettings {
    pub show_axes: bool,
    pub show_origin: bool,
    pub show_statistics: bool,
    pub wireframe_mode: bool,
    pub transparency: f32,
}

/// Precision settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecisionSettings {
    pub decimal_places: u32,
    pub angle_precision: u32,
    pub tolerance: f64,
}

/// Real-time system status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    /// AI connection status
    pub ai_connected: bool,

    /// AI provider info
    pub ai_provider: String,

    /// AI model
    pub ai_model: String,

    /// WebSocket connection status
    pub websocket_connected: bool,

    /// Number of objects in scene
    pub object_count: usize,

    /// Memory usage (MB)
    pub memory_usage: f64,

    /// CPU usage (%)
    pub cpu_usage: f32,

    /// FPS
    pub fps: f32,

    /// Pending operations
    pub pending_operations: usize,

    /// Error count
    pub error_count: usize,

    /// Last error message
    pub last_error: Option<String>,
}

impl Default for SystemContext {
    fn default() -> Self {
        Self {
            session: SessionContext {
                session_id: Uuid::new_v4().to_string(),
                current_user: UserInfo {
                    id: "user".to_string(),
                    name: "User".to_string(),
                    role: "owner".to_string(),
                    status: UserStatus::Active,
                    connected_at: 0,
                    last_active: 0,
                    cursor_position: None,
                    selected_objects: vec![],
                },
                connected_users: vec![],
                recent_disconnections: vec![],
                started_at: 0,
                metadata: HashMap::new(),
            },
            workflow: WorkflowContext {
                current_stage: WorkflowStage::Part,
                current_step: None,
                available_stages: vec![
                    WorkflowStage::Sketch,
                    WorkflowStage::Part,
                    WorkflowStage::Assembly,
                    WorkflowStage::Drawing,
                    WorkflowStage::Simulation,
                    WorkflowStage::Manufacturing,
                ],
                history: vec![],
                project_name: None,
                active_tool: None,
                tool_parameters: HashMap::new(),
            },
            commands: CommandContext {
                creation_commands: vec![
                    CommandInfo {
                        name: "create_box".to_string(),
                        description: "Create a box/cube primitive".to_string(),
                        examples: vec![
                            "create a box".to_string(),
                            "make a cube with width 5".to_string(),
                            "create a box 10x20x30".to_string(),
                        ],
                        required_params: vec![],
                        optional_params: vec![
                            ParameterInfo {
                                name: "width".to_string(),
                                param_type: "number".to_string(),
                                description: "Width of the box".to_string(),
                                default_value: Some(serde_json::json!(10.0)),
                                constraints: Some(serde_json::json!({"min": 0.1, "max": 1000})),
                            },
                            ParameterInfo {
                                name: "height".to_string(),
                                param_type: "number".to_string(),
                                description: "Height of the box".to_string(),
                                default_value: Some(serde_json::json!(10.0)),
                                constraints: Some(serde_json::json!({"min": 0.1, "max": 1000})),
                            },
                            ParameterInfo {
                                name: "depth".to_string(),
                                param_type: "number".to_string(),
                                description: "Depth of the box".to_string(),
                                default_value: Some(serde_json::json!(10.0)),
                                constraints: Some(serde_json::json!({"min": 0.1, "max": 1000})),
                            },
                        ],
                        category: "geometry".to_string(),
                        shortcut: Some("B".to_string()),
                    },
                    CommandInfo {
                        name: "create_sphere".to_string(),
                        description: "Create a sphere primitive".to_string(),
                        examples: vec![
                            "create a sphere".to_string(),
                            "make a sphere with radius 5".to_string(),
                            "add a ball".to_string(),
                        ],
                        required_params: vec![],
                        optional_params: vec![ParameterInfo {
                            name: "radius".to_string(),
                            param_type: "number".to_string(),
                            description: "Radius of the sphere".to_string(),
                            default_value: Some(serde_json::json!(5.0)),
                            constraints: Some(serde_json::json!({"min": 0.1, "max": 1000})),
                        }],
                        category: "geometry".to_string(),
                        shortcut: Some("S".to_string()),
                    },
                    CommandInfo {
                        name: "create_cylinder".to_string(),
                        description: "Create a cylinder primitive".to_string(),
                        examples: vec![
                            "create a cylinder".to_string(),
                            "make a cylinder with radius 3 and height 10".to_string(),
                            "add a tube".to_string(),
                        ],
                        required_params: vec![],
                        optional_params: vec![
                            ParameterInfo {
                                name: "radius".to_string(),
                                param_type: "number".to_string(),
                                description: "Radius of the cylinder".to_string(),
                                default_value: Some(serde_json::json!(5.0)),
                                constraints: Some(serde_json::json!({"min": 0.1, "max": 1000})),
                            },
                            ParameterInfo {
                                name: "height".to_string(),
                                param_type: "number".to_string(),
                                description: "Height of the cylinder".to_string(),
                                default_value: Some(serde_json::json!(10.0)),
                                constraints: Some(serde_json::json!({"min": 0.1, "max": 1000})),
                            },
                        ],
                        category: "geometry".to_string(),
                        shortcut: Some("C".to_string()),
                    },
                ],
                modification_commands: vec![],
                query_commands: vec![CommandInfo {
                    name: "count_objects".to_string(),
                    description: "Count objects in the scene".to_string(),
                    examples: vec![
                        "how many objects are there?".to_string(),
                        "count objects".to_string(),
                    ],
                    required_params: vec![],
                    optional_params: vec![],
                    category: "query".to_string(),
                    shortcut: None,
                }],
                workflow_commands: vec![],
                system_commands: vec![],
                aliases: HashMap::from([
                    ("box".to_string(), "create_box".to_string()),
                    ("cube".to_string(), "create_box".to_string()),
                    ("sphere".to_string(), "create_sphere".to_string()),
                    ("ball".to_string(), "create_sphere".to_string()),
                    ("cylinder".to_string(), "create_cylinder".to_string()),
                    ("tube".to_string(), "create_cylinder".to_string()),
                ]),
            },
            capabilities: SystemCapabilities {
                geometry_types: vec![
                    "box".to_string(),
                    "sphere".to_string(),
                    "cylinder".to_string(),
                    "cone".to_string(),
                    "torus".to_string(),
                ],
                boolean_operations: vec![
                    "union".to_string(),
                    "intersection".to_string(),
                    "difference".to_string(),
                ],
                export_formats: vec!["STL".to_string(), "OBJ".to_string()],
                import_formats: vec!["STL".to_string(), "OBJ".to_string()],
                analysis_features: vec![],
                collaboration_features: vec![
                    "real-time sync".to_string(),
                    "multi-user editing".to_string(),
                    "cursor tracking".to_string(),
                ],
                ai_features: vec![
                    "natural language commands".to_string(),
                    "scene awareness".to_string(),
                    "context understanding".to_string(),
                ],
                version_control: true,
                real_time_sync: true,
            },
            environment: EnvironmentContext {
                unit_system: "metric".to_string(),
                grid: GridInfo {
                    enabled: true,
                    spacing: 10.0,
                    major_lines: 5,
                    visible: true,
                },
                snap_settings: SnapSettings {
                    grid_snap: true,
                    object_snap: true,
                    angle_snap: false,
                    snap_tolerance: 5.0,
                },
                display: DisplaySettings {
                    show_axes: true,
                    show_origin: true,
                    show_statistics: true,
                    wireframe_mode: false,
                    transparency: 1.0,
                },
                precision: PrecisionSettings {
                    decimal_places: 3,
                    angle_precision: 1,
                    tolerance: 0.001,
                },
                theme: "dark".to_string(),
                language: "en".to_string(),
            },
            status: SystemStatus {
                ai_connected: false,
                ai_provider: "ollama".to_string(),
                ai_model: "phi3:mini".to_string(),
                websocket_connected: false,
                object_count: 0,
                memory_usage: 0.0,
                cpu_usage: 0.0,
                fps: 60.0,
                pending_operations: 0,
                error_count: 0,
                last_error: None,
            },
        }
    }
}
