//! Session management types
//!
//! Provides data structures for managing user sessions and collaboration.

use crate::{CADObject, ObjectId, Position3D, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Complete session state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Session identifier
    pub id: ObjectId,
    /// Session name
    pub name: String,
    /// Session owner user ID
    pub owner_id: String,
    /// Objects in the session
    pub objects: HashMap<ObjectId, CADObject>,
    /// Command history
    pub history: VecDeque<HistoryEntry>,
    /// Current history index (for undo/redo)
    pub history_index: usize,
    /// Creation timestamp
    pub created_at: Timestamp,
    /// Last modification timestamp
    pub modified_at: Timestamp,
    /// Active users
    pub active_users: Vec<UserInfo>,
    /// Session settings
    pub settings: SessionSettings,
    /// Session metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Active sketch planes
    pub sketch_planes: HashMap<String, SketchPlaneInfo>,
    /// Current active sketch plane ID
    pub active_sketch_plane: Option<String>,
    /// Orientation cube state
    pub orientation_cube: OrientationCubeState,
    /// Sketch state for tracking drawing operations
    pub sketch_state: SketchState,
}

/// Orientation cube state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrientationCubeState {
    /// Current view name (e.g., "TOP", "FRONT", "RIGHT", "ISO")
    pub current_view: String,
    /// Rotation angles in degrees (rotX, rotY)
    pub rotation: (f64, f64),
    /// Which face is currently visible/active
    pub active_face: CubeFace,
    /// Camera position for this view
    pub camera_position: [f64; 3],
    /// Camera up vector
    pub camera_up: [f64; 3],
    /// Camera target
    pub camera_target: [f64; 3],
}

/// Cube face identifiers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CubeFace {
    /// Top face — XY plane with +Z normal.
    Top,
    /// Bottom face — XY plane with -Z normal.
    Bottom,
    /// Front face — XZ plane with -Y normal.
    Front,
    /// Back face — XZ plane with +Y normal.
    Back,
    /// Right face — YZ plane with +X normal.
    Right,
    /// Left face — YZ plane with -X normal.
    Left,
}

/// Sketch state for tracking drawing operations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SketchState {
    /// Center point for circle being drawn (during two-click circle creation)
    pub circle_center: Option<[f64; 3]>,
    /// Start point for line being drawn
    pub line_start: Option<[f64; 3]>,
    /// Start point for rectangle being drawn
    pub rect_start: Option<[f64; 3]>,
    /// Active drawing tool
    pub active_tool: Option<String>,
}

/// Sketch plane information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchPlaneInfo {
    /// Unique plane identifier
    pub id: String,
    /// Plane type (XY, XZ, YZ, Custom)
    pub plane_type: String,
    /// Plane position in 3D space
    pub position: [f64; 3],
    /// Plane normal vector
    pub normal: [f64; 3],
    /// Size of the plane for visualization
    pub size: f64,
    /// Name of the sketch plane
    pub name: String,
    /// Sketch entities on this plane
    pub entities: Vec<String>, // Entity IDs
    /// Creation timestamp
    pub created_at: Timestamp,
}

/// User information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserInfo {
    /// User identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// User color for collaboration
    pub color: [f32; 4],
    /// Last activity timestamp
    pub last_activity: Timestamp,
    /// User role
    pub role: UserRole,
    /// Cursor position (for collaboration)
    pub cursor_position: Option<Position3D>,
    /// Selected objects
    pub selected_objects: Vec<ObjectId>,
}

/// User roles in session
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserRole {
    /// Can view only
    Viewer,
    /// Can edit objects
    Editor,
    /// Full control
    Owner,
}

/// Session settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSettings {
    /// Enable auto-save
    pub auto_save: bool,
    /// Auto-save interval in seconds
    pub auto_save_interval: u64,
    /// Maximum history entries
    pub max_history: usize,
    /// Enable collaboration
    pub collaboration_enabled: bool,
    /// Allow anonymous users
    pub allow_anonymous: bool,
    /// Session timeout in seconds
    pub timeout_seconds: Option<u64>,
    /// Grid settings
    pub grid: GridSettings,
    /// Units
    pub units: Units,
}

/// Grid display settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridSettings {
    /// Show grid
    pub visible: bool,
    /// Grid spacing
    pub spacing: f32,
    /// Major grid lines every N lines
    pub major_lines: u32,
    /// Snap to grid
    pub snap_enabled: bool,
}

/// Measurement units
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Units {
    /// Millimeters
    Millimeters,
    /// Centimeters
    Centimeters,
    /// Meters
    Meters,
    /// Inches
    Inches,
    /// Feet
    Feet,
}

/// History entry for undo/redo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Entry ID
    pub id: ObjectId,
    /// Command that was executed
    pub command: crate::AICommand,
    /// Timestamp
    pub timestamp: Timestamp,
    /// User who executed
    pub user_id: Option<String>,
    /// Undo data (serialized state)
    pub undo_data: Option<serde_json::Value>,
    /// Redo data
    pub redo_data: Option<serde_json::Value>,
    /// Description
    pub description: String,
}

/// Collaboration event
///
/// The ObjectCreated variant carries a full CADObject and is larger than the
/// other variants, but these events are rare broadcast payloads rather than
/// hot-path values; keeping the object inline avoids indirection at the
/// match/serialize call sites in session-manager.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CollaborationEvent {
    /// User joined session
    UserJoined {
        /// User that joined the session.
        user: UserInfo,
    },
    /// User left session
    UserLeft {
        /// Identifier of the user that left.
        user_id: String,
    },
    /// Object was created
    ObjectCreated {
        /// Newly created object.
        object: CADObject,
        /// Identifier of the user that created the object.
        user_id: String,
    },
    /// Object was modified
    ObjectModified {
        /// Identifier of the modified object.
        object_id: ObjectId,
        /// Identifier of the user that performed the modification.
        user_id: String,
    },
    /// Object was deleted
    ObjectDeleted {
        /// Identifier of the deleted object.
        object_id: ObjectId,
        /// Identifier of the user that deleted the object.
        user_id: String,
    },
    /// Cursor moved
    CursorMoved {
        /// Identifier of the user whose cursor moved.
        user_id: String,
        /// New cursor position in world space.
        position: Position3D,
    },
    /// Selection changed
    SelectionChanged {
        /// Identifier of the user whose selection changed.
        user_id: String,
        /// Currently selected object IDs.
        objects: Vec<ObjectId>,
    },
}

impl SessionState {
    /// Create new session
    pub fn new(id: ObjectId, owner_id: String) -> Self {
        let now = crate::unix_millis_now();

        Self {
            id,
            name: format!("Session {}", id),
            owner_id,
            objects: HashMap::new(),
            history: VecDeque::with_capacity(100),
            history_index: 0,
            created_at: now,
            modified_at: now,
            active_users: Vec::new(),
            settings: SessionSettings::default(),
            metadata: HashMap::new(),
            sketch_planes: HashMap::new(),
            active_sketch_plane: None,
            orientation_cube: OrientationCubeState::default(),
            sketch_state: SketchState::default(),
        }
    }

    /// Add object to session
    pub fn add_object(&mut self, object: CADObject) {
        self.objects.insert(object.id, object);
        self.update_modified_time();
    }

    /// Remove object from session
    pub fn remove_object(&mut self, object_id: &ObjectId) -> Option<CADObject> {
        let removed = self.objects.remove(object_id);
        if removed.is_some() {
            self.update_modified_time();
        }
        removed
    }

    /// Get object by ID
    pub fn get_object(&self, object_id: &ObjectId) -> Option<&CADObject> {
        self.objects.get(object_id)
    }

    /// Get mutable object by ID
    pub fn get_object_mut(&mut self, object_id: &ObjectId) -> Option<&mut CADObject> {
        // touch the timestamp first — the borrow ends here
        self.update_modified_time();

        // now it’s safe to take and return the &mut CADObject
        self.objects.get_mut(object_id)
    }

    /// Add history entry
    pub fn add_history(&mut self, entry: HistoryEntry) {
        // Remove any entries after current index (for redo)
        self.history.truncate(self.history_index);

        // Add new entry
        self.history.push_back(entry);
        self.history_index = self.history.len();

        // Limit history size
        while self.history.len() > self.settings.max_history {
            self.history.pop_front();
            self.history_index = self.history_index.saturating_sub(1);
        }

        self.update_modified_time();
    }

    /// Check if can undo
    pub fn can_undo(&self) -> bool {
        self.history_index > 0
    }

    /// Check if can redo
    pub fn can_redo(&self) -> bool {
        self.history_index < self.history.len()
    }

    /// Get current history entry
    pub fn current_history(&self) -> Option<&HistoryEntry> {
        if self.history_index > 0 {
            self.history.get(self.history_index - 1)
        } else {
            None
        }
    }

    fn update_modified_time(&mut self) {
        self.modified_at = crate::unix_millis_now();
    }

    /// Add a sketch plane to the session
    pub fn add_sketch_plane(&mut self, plane: SketchPlaneInfo) -> String {
        let plane_id = plane.id.clone();
        self.sketch_planes.insert(plane_id.clone(), plane);
        self.update_modified_time();
        plane_id
    }

    /// Remove a sketch plane from the session
    pub fn remove_sketch_plane(&mut self, plane_id: &str) -> Option<SketchPlaneInfo> {
        let removed = self.sketch_planes.remove(plane_id);
        if removed.is_some() {
            // If this was the active plane, clear it
            if self.active_sketch_plane.as_ref() == Some(&plane_id.to_string()) {
                self.active_sketch_plane = None;
            }
            self.update_modified_time();
        }
        removed
    }

    /// Set the active sketch plane
    pub fn set_active_sketch_plane(&mut self, plane_id: Option<String>) {
        self.active_sketch_plane = plane_id;
        self.update_modified_time();
    }

    /// Get a sketch plane by ID
    pub fn get_sketch_plane(&self, plane_id: &str) -> Option<&SketchPlaneInfo> {
        self.sketch_planes.get(plane_id)
    }

    /// Get mutable sketch plane by ID
    pub fn get_sketch_plane_mut(&mut self, plane_id: &str) -> Option<&mut SketchPlaneInfo> {
        self.update_modified_time();
        self.sketch_planes.get_mut(plane_id)
    }

    /// Add entity to sketch plane
    pub fn add_entity_to_sketch(&mut self, plane_id: &str, entity_id: String) -> bool {
        if let Some(plane) = self.get_sketch_plane_mut(plane_id) {
            plane.entities.push(entity_id);
            true
        } else {
            false
        }
    }
}

impl UserInfo {
    /// Create new user
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            color: [0.5, 0.5, 1.0, 1.0], // Default blue
            last_activity: crate::unix_millis_now(),
            role: UserRole::Editor,
            cursor_position: None,
            selected_objects: Vec::new(),
        }
    }

    /// Update activity timestamp
    pub fn update_activity(&mut self) {
        self.last_activity = crate::unix_millis_now();
    }

    /// Check if user is active
    pub fn is_active(&self, timeout_ms: u64) -> bool {
        let now = crate::unix_millis_now();

        now - self.last_activity < timeout_ms
    }
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            auto_save: true,
            auto_save_interval: 300, // 5 minutes
            max_history: 100,
            collaboration_enabled: true,
            allow_anonymous: false,
            timeout_seconds: Some(3600), // 1 hour
            grid: GridSettings::default(),
            units: Units::Millimeters,
        }
    }
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            visible: true,
            spacing: 10.0,
            major_lines: 5,
            snap_enabled: true,
        }
    }
}

impl Default for OrientationCubeState {
    fn default() -> Self {
        Self {
            current_view: "ISO".to_string(),
            rotation: (35.0, 45.0), // Isometric view
            active_face: CubeFace::Front,
            camera_position: [100.0, 100.0, 100.0],
            camera_up: [0.0, 0.0, 1.0],
            camera_target: [0.0, 0.0, 0.0],
        }
    }
}

impl Units {
    /// Convert value to millimeters
    pub fn to_mm(&self, value: f32) -> f32 {
        match self {
            Units::Millimeters => value,
            Units::Centimeters => value * 10.0,
            Units::Meters => value * 1000.0,
            Units::Inches => value * 25.4,
            Units::Feet => value * 304.8,
        }
    }

    /// Convert a value expressed in millimeters into this unit.
    pub fn convert_from_mm(&self, value: f32) -> f32 {
        match self {
            Units::Millimeters => value,
            Units::Centimeters => value / 10.0,
            Units::Meters => value / 1000.0,
            Units::Inches => value / 25.4,
            Units::Feet => value / 304.8,
        }
    }

    /// Get unit suffix
    pub fn suffix(&self) -> &'static str {
        match self {
            Units::Millimeters => "mm",
            Units::Centimeters => "cm",
            Units::Meters => "m",
            Units::Inches => "in",
            Units::Feet => "ft",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_state() {
        let mut session = SessionState::new(ObjectId::new_v4(), "test-owner".to_string());
        assert!(session.objects.is_empty());

        let object = CADObject {
            id: ObjectId::new_v4(),
            name: "Test".to_string(),
            mesh: crate::Mesh::new(),
            analytical_geometry: None,
            cached_meshes: HashMap::new(),
            transform: crate::Transform3D::identity(),
            material: crate::geometry::MaterialProperties::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            metadata: HashMap::new(),
            created_at: 0,
            modified_at: 0,
        };

        let object_id = object.id;
        session.add_object(object);

        assert_eq!(session.objects.len(), 1);
        assert!(session.get_object(&object_id).is_some());
    }

    #[test]
    fn test_user_activity() {
        let mut user = UserInfo::new("user1".to_string(), "Test User".to_string());
        assert!(user.is_active(1000)); // Active within 1 second

        user.last_activity = 0; // Set to epoch
        assert!(!user.is_active(1000)); // Not active
    }

    #[test]
    fn test_unit_conversion() {
        assert_eq!(Units::Inches.to_mm(1.0), 25.4);
        assert_eq!(Units::Millimeters.convert_from_mm(25.4), 25.4);
        assert_eq!(Units::Inches.convert_from_mm(25.4), 1.0);
    }
}
