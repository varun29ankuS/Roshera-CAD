//! ClientMessage/ServerMessage Protocol Definitions
//!
//! IMPORTANT: This is the APPLICATION-LEVEL PROTOCOL, not the transport layer.
//! - Protocol: ClientMessage/ServerMessage (defined here)
//! - Transport: WebSocket (just the delivery mechanism)
//!
//! This defines the comprehensive message protocol for real-time communication
//! with all backend modules including geometry, timeline, export, AI, and sessions.
//!
//! The protocol is transmitted over WebSocket at the /ws endpoint, but WebSocket
//! is just the transport. The actual protocol is ClientMessage/ServerMessage.

use serde::{Deserialize, Serialize};
use shared_types::commands::CommandContext;
use shared_types::session::UserInfo;
use shared_types::*;
use uuid::Uuid;

// Use the geometry commands QueryType for geometry queries
use shared_types::geometry_commands::QueryType as GeometryQueryType;

// ============================================================================
// SUPPORTING TYPES (must be defined before the message types that use them)
// ============================================================================

/// Timeline operation payload carried by `TimelineWSCommand::ExecuteOperation`.
/// Lives here (not in a separate `timeline_handlers` module) because this is
/// the only consumer — the legacy `timeline_handlers.rs` dispatch shim was
/// orphaned by the move to `message_handlers.rs` and was deleted in #29.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation_type")]
pub enum TimelineOperation {
    CreatePrimitive {
        primitive_type: String,
        parameters: serde_json::Value,
    },
    Transform {
        entity_id: String,
        transformation: [[f64; 4]; 4],
    },
    BooleanUnion {
        operands: Vec<String>,
    },
    BooleanIntersection {
        operands: Vec<String>,
    },
    BooleanDifference {
        target: String,
        tools: Vec<String>,
    },
    Delete {
        entities: Vec<String>,
    },
}

/// Geometry WebSocket commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum GeometryWSCommand {
    CreatePrimitive {
        primitive_type: PrimitiveType,
        parameters: ShapeParameters,
    },
    BooleanOperation {
        operation: BooleanOp,
        operands: Vec<ObjectId>,
    },
    Transform {
        object_id: ObjectId,
        transform: Transform3D,
    },
    Delete {
        object_ids: Vec<ObjectId>,
    },
    Query {
        object_id: ObjectId,
        query_type: GeometryQueryType,
    },
}

/// Timeline WebSocket commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum TimelineWSCommand {
    ExecuteOperation {
        operation: TimelineOperation,
    },
    Undo {
        steps: Option<usize>,
    },
    Redo {
        steps: Option<usize>,
    },
    CreateBranch {
        name: String,
        from_point: Option<usize>,
    },
    SwitchBranch {
        branch_name: String,
    },
    MergeBranch {
        source: String,
        target: String,
        strategy: MergeStrategy,
    },
}

/// Export WebSocket commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum ExportWSCommand {
    ExportSTL {
        object_ids: Vec<ObjectId>,
        format: STLFormat,
    },
    ExportOBJ {
        object_ids: Vec<ObjectId>,
        include_materials: bool,
    },
    ExportROS {
        filename: String,
        options: ROSExportOptions,
    },
    ExportSTEP {
        object_ids: Vec<ObjectId>,
    },
}

/// AI WebSocket commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum AIWSCommand {
    ProcessCommand {
        text: String,
        context: Option<CommandContext>,
    },
    GenerateResponse {
        prompt: String,
        model: Option<String>,
    },
    SuggestNext {
        current_state: Vec<ObjectId>,
    },
}

/// Session WebSocket commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum SessionWSCommand {
    CreateSession {
        name: String,
        description: Option<String>,
    },
    JoinSession {
        session_id: String,
    },
    LeaveSession,
    ShareObject {
        object_id: ObjectId,
        permissions: Vec<Permission>,
    },
    UpdatePresence {
        cursor_position: Option<Position3D>,
        selected_objects: Vec<ObjectId>,
    },
}

/// Subscription topics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubscriptionTopic {
    GeometryUpdates,
    TimelineUpdates,
    SessionUpdates,
    AIResponses,
    SystemEvents,
    ErrorEvents,
    AllEvents,
}

/// Object filter for queries (moved here to be defined before WSQueryType)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectFilter {
    pub object_type: Option<ObjectType>,
    pub created_after: Option<Timestamp>,
    pub created_before: Option<Timestamp>,
    pub modified_after: Option<Timestamp>,
    pub modified_before: Option<Timestamp>,
    pub tags: Option<Vec<String>>,
}

/// Query types for retrieving data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "query_type")]
pub enum WSQueryType {
    GetObject {
        object_id: ObjectId,
    },
    ListObjects {
        filter: Option<ObjectFilter>,
        limit: Option<usize>,
        offset: Option<usize>,
    },
    GetTimelineState,
    GetSessionInfo {
        session_id: String,
    },
    GetSystemStatus,
    GetCapabilities,
    GetMetrics,
    SearchObjects {
        query: String,
        limit: Option<usize>,
    },
}

/// Merge strategies for timeline branches
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MergeStrategy {
    PreferSource,
    PreferTarget,
    Manual,
}

/// Audio format for voice processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AudioFormat {
    WAV,
    MP3,
    OGG,
    WEBM,
}

// ============================================================================
// MAIN MESSAGE TYPES
// ============================================================================

/// ClientMessage: The APPLICATION PROTOCOL for client-to-server communication
///
/// This is NOT the "WebSocket protocol" - WebSocket is just the transport layer.
/// This enum defines the actual protocol messages that clients send to the server.
/// All messages are sent over WebSocket transport at the /ws endpoint.
///
/// Vision commands are integrated here under AICommand, not as a separate protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    // Authentication
    Authenticate {
        token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Geometry Engine commands
    GeometryCommand {
        command: GeometryWSCommand,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Timeline Engine commands
    TimelineCommand {
        command: TimelineWSCommand,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Export Engine commands
    ExportCommand {
        command: ExportWSCommand,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // AI Integration commands
    AICommand {
        command: AIWSCommand,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Session Manager commands
    SessionCommand {
        command: SessionWSCommand,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Subscription management
    Subscribe {
        topics: Vec<SubscriptionTopic>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    Unsubscribe {
        topics: Vec<SubscriptionTopic>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Query operations
    Query {
        query_type: serde_json::Value, // Temporarily use Value to work around serialization issue
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Keepalive
    Ping {
        timestamp: u64,
    },
}

/// ServerMessage: The APPLICATION PROTOCOL for server-to-client communication
///
/// This is the server's response protocol, NOT "WebSocket messages".
/// These are structured protocol messages sent from server to client.
/// All messages are sent over WebSocket transport at the /ws endpoint.
///
/// Responses to vision commands come through the standard Success/Error variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerMessage {
    // Connection management
    Welcome {
        connection_id: String,
        server_version: String,
        capabilities: Vec<String>,
    },

    // Authentication responses
    Authenticated {
        user_id: String,
        permissions: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    AuthenticationFailed {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Command responses
    Success {
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    Error {
        error_code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Update notifications
    GeometryUpdate {
        update: GeometryUpdate,
    },

    TimelineUpdate {
        update: TimelineUpdate,
    },

    SessionUpdate {
        update: SessionUpdate,
    },

    // Export results
    ExportComplete {
        result: ExportResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // AI responses
    AIResponse {
        response: AIResponse,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // System events
    SystemEvent {
        event_type: String,
        data: serde_json::Value,
    },

    // Progress updates
    Progress {
        operation: String,
        percentage: f32,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // Keepalive response
    Pong {
        timestamp: u64,
    },
}

// ============================================================================
// UPDATE TYPES (for notifications)
// ============================================================================

/// Geometry update types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "update_type")]
pub enum GeometryUpdate {
    Created {
        object: CADObject,
    },
    Modified {
        object_id: ObjectId,
        changes: ObjectChanges,
    },
    Deleted {
        object_ids: Vec<ObjectId>,
    },
    Tessellated {
        object_id: ObjectId,
        mesh: Mesh,
    },
}

/// Timeline update types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "update_type")]
pub enum TimelineUpdate {
    OperationExecuted {
        operation: TimelineOperation,
        result_ids: Vec<ObjectId>,
    },
    UndoPerformed {
        steps: usize,
    },
    RedoPerformed {
        steps: usize,
    },
    BranchCreated {
        name: String,
    },
    BranchSwitched {
        from: String,
        to: String,
    },
}

/// Export result types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "format")]
pub enum ExportResult {
    STL {
        filename: String,
        size_bytes: usize,
    },
    OBJ {
        obj_file: String,
        mtl_file: Option<String>,
        size_bytes: usize,
    },
    ROS {
        filename: String,
        encrypted: bool,
        size_bytes: usize,
    },
    STEP {
        filename: String,
        size_bytes: usize,
    },
}

/// AI response types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "response_type")]
pub enum AIResponse {
    CommandExecuted {
        command: String,
        results: Vec<ObjectId>,
    },
    TextResponse {
        text: String,
        confidence: f32,
    },
    VoiceTranscribed {
        text: String,
        confidence: f32,
    },
    Suggestion {
        suggestions: Vec<String>,
    },
    Error {
        message: String,
    },
}

/// Session update types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "update_type")]
pub enum SessionUpdate {
    UserJoined {
        user_id: String,
        user_info: UserInfo,
    },
    UserLeft {
        user_id: String,
    },
    ObjectShared {
        object_id: ObjectId,
        shared_by: String,
    },
    PresenceUpdated {
        user_id: String,
        cursor_position: Option<Position3D>,
        selected_objects: Vec<ObjectId>,
    },
    SessionEnded {
        reason: String,
    },
}

// ============================================================================
// SUPPORTING TYPES (referenced by other types)
// ============================================================================

// ObjectFilter moved earlier in the file to be defined before WSQueryType

/// Object changes for updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectChanges {
    pub transform: Option<Transform3D>,
    pub material: Option<MaterialProperties>,
    pub visibility: Option<bool>,
    pub tags: Option<Vec<String>>,
}

/// Permission types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Permission {
    Read,
    Write,
    Delete,
    Share,
}

/// ROS export options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ROSExportOptions {
    pub encrypt: bool,
    pub password: Option<String>,
    pub track_ai: bool,
    pub sign: bool,
}

/// STL format options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum STLFormat {
    Binary,
    ASCII,
}
