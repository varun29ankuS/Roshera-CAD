//! Delta updates for efficient real-time broadcasting
//!
//! This module provides delta compression and updates to minimize
//! network traffic for real-time collaboration.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use shared_types::session::UserInfo;
use shared_types::{AICommand, CADObject, HistoryEntry, ObjectId, SessionError, SessionState};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, trace};
use uuid::Uuid;

/// Types of changes in a delta update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeltaType {
    /// Object was added
    Added,
    /// Object was modified
    Modified,
    /// Object was removed
    Removed,
    /// Object was moved/transformed
    Transformed,
}

/// Delta update for a single object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectDelta {
    /// Object ID
    pub id: ObjectId,
    /// Type of change
    pub delta_type: DeltaType,
    /// Changed properties (only for Modified)
    pub changes: Option<serde_json::Value>,
    /// Full object (for Added)
    pub object: Option<CADObject>,
    /// Transform data (for Transformed)
    pub transform: Option<serde_json::Value>,
}

/// Delta update for session state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDelta {
    /// Session ID
    pub session_id: Uuid,
    /// Sequence number for ordering
    pub sequence: u64,
    /// Timestamp
    pub timestamp: u64,
    /// Object changes
    pub object_deltas: Vec<ObjectDelta>,
    /// Timeline changes
    pub timeline_delta: Option<TimelineDelta>,
    /// Metadata changes
    pub metadata_changes: Option<HashMap<String, serde_json::Value>>,
    /// Active users changes
    pub user_changes: Option<UserChanges>,
    /// Settings changes
    pub settings_changes: Option<serde_json::Value>,
}

/// Timeline delta updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineDelta {
    /// New history entries added
    pub new_entries: Vec<HistoryEntry>,
    /// Current branch change
    pub branch_change: Option<String>,
    /// History index change
    pub index_change: Option<usize>,
}

/// User changes in session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserChanges {
    /// Users who joined
    pub joined: Vec<UserInfo>,
    /// Users who left (by ID)
    pub left: Vec<String>,
    /// User cursor/selection updates
    pub updates: HashMap<String, serde_json::Value>,
}

/// Delta tracker for a session
#[derive(Debug)]
pub struct DeltaTracker {
    /// Session ID
    session_id: Uuid,
    /// Last known state hash
    last_state_hash: u64,
    /// Sequence counter
    sequence: u64,
    /// Object hashes for change detection
    object_hashes: Arc<DashMap<ObjectId, u64>>,
    /// Metadata hashes
    metadata_hashes: Arc<DashMap<String, u64>>,
    /// Active users (by ID)
    active_users: Arc<DashMap<String, UserInfo>>,
}

impl DeltaTracker {
    /// Create new delta tracker
    pub fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            last_state_hash: 0,
            sequence: 0,
            object_hashes: Arc::new(DashMap::new()),
            metadata_hashes: Arc::new(DashMap::new()),
            active_users: Arc::new(DashMap::new()),
        }
    }

    /// Compute delta between two session states
    pub fn compute_delta(
        &mut self,
        old_state: &SessionState,
        new_state: &SessionState,
    ) -> Result<SessionDelta, SessionError> {
        self.sequence += 1;

        let mut delta = SessionDelta {
            session_id: self.session_id,
            sequence: self.sequence,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            object_deltas: Vec::new(),
            timeline_delta: None,
            metadata_changes: None,
            user_changes: None,
            settings_changes: None,
        };

        // Compute object deltas
        delta.object_deltas = self.compute_object_deltas(&old_state.objects, &new_state.objects)?;

        // Compute timeline delta
        delta.timeline_delta = self.compute_timeline_delta(old_state, new_state);

        // Compute metadata changes
        delta.metadata_changes =
            self.compute_metadata_changes(&old_state.metadata, &new_state.metadata);

        // Compute user changes
        delta.user_changes =
            self.compute_user_changes(&old_state.active_users, &new_state.active_users);

        // Compute settings changes
        if serde_json::to_value(&old_state.settings)? != serde_json::to_value(&new_state.settings)?
        {
            delta.settings_changes = Some(serde_json::to_value(&new_state.settings)?);
        }

        Ok(delta)
    }

    /// Compute object deltas
    fn compute_object_deltas(
        &self,
        old_objects: &HashMap<ObjectId, CADObject>,
        new_objects: &HashMap<ObjectId, CADObject>,
    ) -> Result<Vec<ObjectDelta>, SessionError> {
        let mut deltas = Vec::new();

        // Find added and modified objects
        for (id, new_obj) in new_objects {
            let new_hash = hash_object(new_obj);

            if let Some(old_obj) = old_objects.get(id) {
                let old_hash = self
                    .object_hashes
                    .get(id)
                    .map(|h| *h)
                    .unwrap_or_else(|| hash_object(old_obj));

                if old_hash != new_hash {
                    // Object modified
                    let changes = compute_object_changes(old_obj, new_obj)?;
                    let is_transform = is_transform_only(&changes);

                    deltas.push(ObjectDelta {
                        id: *id,
                        delta_type: if is_transform {
                            DeltaType::Transformed
                        } else {
                            DeltaType::Modified
                        },
                        changes: Some(changes.clone()),
                        object: None,
                        transform: if is_transform {
                            extract_transform(&changes)
                        } else {
                            None
                        },
                    });
                }
            } else {
                // Object added
                deltas.push(ObjectDelta {
                    id: *id,
                    delta_type: DeltaType::Added,
                    changes: None,
                    object: Some(new_obj.clone()),
                    transform: None,
                });
            }

            // Update hash
            self.object_hashes.insert(*id, new_hash);
        }

        // Find removed objects
        for id in old_objects.keys() {
            if !new_objects.contains_key(id) {
                deltas.push(ObjectDelta {
                    id: *id,
                    delta_type: DeltaType::Removed,
                    changes: None,
                    object: None,
                    transform: None,
                });

                self.object_hashes.remove(id);
            }
        }

        Ok(deltas)
    }

    /// Compute timeline delta
    fn compute_timeline_delta(
        &self,
        old_state: &SessionState,
        new_state: &SessionState,
    ) -> Option<TimelineDelta> {
        let old_len = old_state.history.len();
        let new_len = new_state.history.len();

        if old_len == new_len && old_state.history_index == new_state.history_index {
            return None;
        }

        let mut delta = TimelineDelta {
            new_entries: Vec::new(),
            branch_change: None,
            index_change: None,
        };

        // Check for new history entries
        if new_len > old_len {
            delta.new_entries = new_state.history.iter().skip(old_len).cloned().collect();
        }

        // Check for history index change
        if old_state.history_index != new_state.history_index {
            delta.index_change = Some(new_state.history_index);
        }

        // Branch changes would be tracked here if we had branch info

        Some(delta)
    }

    /// Compute metadata changes
    fn compute_metadata_changes(
        &self,
        old_metadata: &HashMap<String, serde_json::Value>,
        new_metadata: &HashMap<String, serde_json::Value>,
    ) -> Option<HashMap<String, serde_json::Value>> {
        let mut changes = HashMap::new();

        // Find changed and new metadata
        for (key, new_value) in new_metadata {
            let new_hash = hash_value(new_value);

            if let Some(old_value) = old_metadata.get(key) {
                let old_hash = self
                    .metadata_hashes
                    .get(key)
                    .map(|h| *h)
                    .unwrap_or_else(|| hash_value(old_value));

                if old_hash != new_hash {
                    changes.insert(key.clone(), new_value.clone());
                }
            } else {
                changes.insert(key.clone(), new_value.clone());
            }

            self.metadata_hashes.insert(key.clone(), new_hash);
        }

        // Find removed metadata
        for key in old_metadata.keys() {
            if !new_metadata.contains_key(key) {
                changes.insert(key.clone(), serde_json::Value::Null);
                self.metadata_hashes.remove(key);
            }
        }

        if changes.is_empty() {
            None
        } else {
            Some(changes)
        }
    }

    /// Compute user changes
    fn compute_user_changes(
        &self,
        old_users: &[UserInfo],
        new_users: &[UserInfo],
    ) -> Option<UserChanges> {
        let old_ids: HashSet<_> = old_users.iter().map(|u| u.id.clone()).collect();
        let new_ids: HashSet<_> = new_users.iter().map(|u| u.id.clone()).collect();

        let joined: Vec<_> = new_users
            .iter()
            .filter(|u| !old_ids.contains(&u.id))
            .cloned()
            .collect();

        let left: Vec<_> = old_ids.difference(&new_ids).cloned().collect();

        // Update tracking
        for user in &joined {
            self.active_users.insert(user.id.clone(), user.clone());
        }
        for user_id in &left {
            self.active_users.remove(user_id);
        }

        if joined.is_empty() && left.is_empty() {
            None
        } else {
            Some(UserChanges {
                joined,
                left,
                updates: HashMap::new(), // Would track cursor positions etc
            })
        }
    }

    /// Apply delta to session state
    pub fn apply_delta(
        &mut self,
        state: &mut SessionState,
        delta: &SessionDelta,
    ) -> Result<(), SessionError> {
        // Verify sequence
        if delta.sequence <= self.sequence {
            trace!("Skipping delta {} (already applied)", delta.sequence);
            return Ok(());
        }

        debug!("Applying delta {} to session", delta.sequence);

        // Apply object deltas
        for obj_delta in &delta.object_deltas {
            match obj_delta.delta_type {
                DeltaType::Added => {
                    if let Some(object) = &obj_delta.object {
                        state.objects.insert(obj_delta.id, object.clone());
                    }
                }
                DeltaType::Modified => {
                    if let Some(obj) = state.objects.get_mut(&obj_delta.id) {
                        if let Some(changes) = &obj_delta.changes {
                            apply_object_changes(obj, changes)?;
                        }
                    }
                }
                DeltaType::Removed => {
                    state.objects.remove(&obj_delta.id);
                }
                DeltaType::Transformed => {
                    if let Some(obj) = state.objects.get_mut(&obj_delta.id) {
                        if let Some(transform) = &obj_delta.transform {
                            apply_transform(obj, transform)?;
                        }
                    }
                }
            }
        }

        // Apply timeline delta
        if let Some(timeline_delta) = &delta.timeline_delta {
            for entry in &timeline_delta.new_entries {
                state.history.push_back(entry.clone());
            }

            if let Some(index) = timeline_delta.index_change {
                state.history_index = index;
            }
        }

        // Apply metadata changes
        if let Some(metadata_changes) = &delta.metadata_changes {
            for (key, value) in metadata_changes {
                if value.is_null() {
                    state.metadata.remove(key);
                } else {
                    state.metadata.insert(key.clone(), value.clone());
                }
            }
        }

        // Apply user changes
        if let Some(user_changes) = &delta.user_changes {
            // Remove left users
            state
                .active_users
                .retain(|u| !user_changes.left.contains(&u.id));

            // Add joined users
            for user in &user_changes.joined {
                if !state.active_users.iter().any(|u| u.id == user.id) {
                    state.active_users.push(user.clone());
                }
            }
        }

        // Apply settings changes
        if let Some(settings) = &delta.settings_changes {
            state.settings = serde_json::from_value(settings.clone())?;
        }

        // Update modified timestamp
        state.modified_at = delta.timestamp;

        // Update sequence
        self.sequence = delta.sequence;

        Ok(())
    }

    /// Create snapshot delta (full state as delta)
    pub fn create_snapshot(&mut self, state: &SessionState) -> Result<SessionDelta, SessionError> {
        self.sequence += 1;

        // Clear all hashes to force full update
        self.object_hashes.clear();
        self.metadata_hashes.clear();
        self.active_users.clear();

        // Create delta with all objects as "added"
        let object_deltas: Vec<_> = state
            .objects
            .iter()
            .map(|(id, obj)| {
                self.object_hashes.insert(*id, hash_object(obj));
                ObjectDelta {
                    id: *id,
                    delta_type: DeltaType::Added,
                    changes: None,
                    object: Some(obj.clone()),
                    transform: None,
                }
            })
            .collect();

        // Create timeline snapshot
        let timeline_delta = if !state.history.is_empty() {
            Some(TimelineDelta {
                new_entries: state.history.iter().cloned().collect(),
                branch_change: None,
                index_change: Some(state.history_index),
            })
        } else {
            None
        };

        // Store metadata hashes
        for (key, value) in &state.metadata {
            self.metadata_hashes.insert(key.clone(), hash_value(value));
        }

        // Store active users
        for user in &state.active_users {
            self.active_users.insert(user.id.clone(), user.clone());
        }

        Ok(SessionDelta {
            session_id: self.session_id,
            sequence: self.sequence,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            object_deltas,
            timeline_delta,
            metadata_changes: if state.metadata.is_empty() {
                None
            } else {
                Some(state.metadata.clone())
            },
            user_changes: if state.active_users.is_empty() {
                None
            } else {
                Some(UserChanges {
                    joined: state.active_users.clone(),
                    left: Vec::new(),
                    updates: HashMap::new(),
                })
            },
            settings_changes: Some(serde_json::to_value(&state.settings)?),
        })
    }
}

/// Compute hash of an object for change detection
fn hash_object(obj: &CADObject) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash relevant fields
    obj.id.hash(&mut hasher);
    obj.name.hash(&mut hasher);
    obj.visible.hash(&mut hasher);
    obj.locked.hash(&mut hasher);

    // Hash transform
    if let Ok(json) = serde_json::to_string(&obj.transform) {
        json.hash(&mut hasher);
    }

    // Hash material
    if let Ok(json) = serde_json::to_string(&obj.material) {
        json.hash(&mut hasher);
    }

    hasher.finish()
}

/// Compute hash of a JSON value
fn hash_value(value: &serde_json::Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    hasher.finish()
}

/// Compute changes between two objects
fn compute_object_changes(
    old: &CADObject,
    new: &CADObject,
) -> Result<serde_json::Value, SessionError> {
    let mut changes = serde_json::Map::new();

    if old.name != new.name {
        changes.insert(
            "name".to_string(),
            serde_json::Value::String(new.name.clone()),
        );
    }

    if old.visible != new.visible {
        changes.insert("visible".to_string(), serde_json::Value::Bool(new.visible));
    }

    if old.locked != new.locked {
        changes.insert("locked".to_string(), serde_json::Value::Bool(new.locked));
    }

    if serde_json::to_value(&old.transform)? != serde_json::to_value(&new.transform)? {
        changes.insert(
            "transform".to_string(),
            serde_json::to_value(&new.transform)?,
        );
    }

    if serde_json::to_value(&old.material)? != serde_json::to_value(&new.material)? {
        changes.insert("material".to_string(), serde_json::to_value(&new.material)?);
    }

    if old.metadata != new.metadata {
        changes.insert("metadata".to_string(), serde_json::to_value(&new.metadata)?);
    }

    Ok(serde_json::Value::Object(changes))
}

/// Check if changes are transform-only
fn is_transform_only(changes: &serde_json::Value) -> bool {
    if let Some(obj) = changes.as_object() {
        obj.len() == 1 && obj.contains_key("transform")
    } else {
        false
    }
}

/// Extract transform from changes
fn extract_transform(changes: &serde_json::Value) -> Option<serde_json::Value> {
    changes
        .as_object()
        .and_then(|obj| obj.get("transform"))
        .cloned()
}

/// Apply object changes
fn apply_object_changes(
    obj: &mut CADObject,
    changes: &serde_json::Value,
) -> Result<(), SessionError> {
    if let Some(changes_obj) = changes.as_object() {
        for (key, value) in changes_obj {
            match key.as_str() {
                "name" => {
                    if let Some(name) = value.as_str() {
                        obj.name = name.to_string();
                    }
                }
                "visible" => {
                    if let Some(visible) = value.as_bool() {
                        obj.visible = visible;
                    }
                }
                "locked" => {
                    if let Some(locked) = value.as_bool() {
                        obj.locked = locked;
                    }
                }
                "transform" => {
                    obj.transform = serde_json::from_value(value.clone())?;
                }
                "material" => {
                    obj.material = serde_json::from_value(value.clone())?;
                }
                "metadata" => {
                    obj.metadata = serde_json::from_value(value.clone())?;
                }
                _ => {
                    // Ignore unknown fields
                }
            }
        }
    }

    Ok(())
}

/// Apply transform to object
fn apply_transform(obj: &mut CADObject, transform: &serde_json::Value) -> Result<(), SessionError> {
    obj.transform = serde_json::from_value(transform.clone())?;
    Ok(())
}

/// Delta compression for network efficiency
pub mod compression {
    use super::*;
    use flate2::{
        write::{GzDecoder, GzEncoder},
        Compression,
    };
    use std::io::Write;

    /// Compress delta for transmission
    pub fn compress_delta(delta: &SessionDelta) -> Result<Vec<u8>, SessionError> {
        let json = serde_json::to_vec(delta)?;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&json)
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Compression failed: {}", e),
            })?;

        encoder
            .finish()
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Compression failed: {}", e),
            })
    }

    /// Decompress delta
    pub fn decompress_delta(data: &[u8]) -> Result<SessionDelta, SessionError> {
        let mut decoder = GzDecoder::new(Vec::new());
        decoder
            .write_all(data)
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Decompression failed: {}", e),
            })?;

        let json = decoder
            .finish()
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Decompression failed: {}", e),
            })?;

        Ok(serde_json::from_slice(&json)?)
    }
}

/// Batch multiple deltas for efficiency
pub fn batch_deltas(deltas: Vec<SessionDelta>) -> Option<SessionDelta> {
    if deltas.is_empty() {
        return None;
    }

    if deltas.len() == 1 {
        return deltas.into_iter().next();
    }

    let mut batched = SessionDelta {
        session_id: deltas[0].session_id,
        sequence: deltas.last().unwrap().sequence,
        timestamp: deltas.last().unwrap().timestamp,
        object_deltas: Vec::new(),
        timeline_delta: None,
        metadata_changes: None,
        user_changes: None,
        settings_changes: None,
    };

    // Merge object deltas
    let mut object_map: HashMap<ObjectId, ObjectDelta> = HashMap::new();

    for delta in &deltas {
        for obj_delta in &delta.object_deltas {
            object_map.insert(obj_delta.id, obj_delta.clone());
        }
    }

    batched.object_deltas = object_map.into_values().collect();

    // Merge timeline deltas
    let mut all_entries = Vec::new();
    let mut last_index = None;
    let mut last_branch = None;

    for delta in &deltas {
        if let Some(timeline) = &delta.timeline_delta {
            all_entries.extend(timeline.new_entries.clone());

            if timeline.index_change.is_some() {
                last_index = timeline.index_change;
            }

            if timeline.branch_change.is_some() {
                last_branch = timeline.branch_change.clone();
            }
        }
    }

    if !all_entries.is_empty() || last_index.is_some() || last_branch.is_some() {
        batched.timeline_delta = Some(TimelineDelta {
            new_entries: all_entries,
            branch_change: last_branch,
            index_change: last_index,
        });
    }

    // Take latest metadata and settings
    for delta in deltas.iter().rev() {
        if batched.metadata_changes.is_none() && delta.metadata_changes.is_some() {
            batched.metadata_changes = delta.metadata_changes.clone();
        }

        if batched.settings_changes.is_none() && delta.settings_changes.is_some() {
            batched.settings_changes = delta.settings_changes.clone();
        }

        if batched.metadata_changes.is_some() && batched.settings_changes.is_some() {
            break;
        }
    }

    // Merge user changes
    let mut all_joined = Vec::new();
    let mut all_left = Vec::new();
    let mut all_updates = HashMap::new();

    for delta in &deltas {
        if let Some(users) = &delta.user_changes {
            all_joined.extend(users.joined.clone());
            all_left.extend(users.left.clone());
            all_updates.extend(users.updates.clone());
        }
    }

    if !all_joined.is_empty() || !all_left.is_empty() || !all_updates.is_empty() {
        batched.user_changes = Some(UserChanges {
            joined: all_joined,
            left: all_left,
            updates: all_updates,
        });
    }

    Some(batched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared_types::{Mesh, Transform3D};

    #[test]
    fn test_delta_computation() {
        let session_id = Uuid::new_v4();
        let mut tracker = DeltaTracker::new(session_id);

        // Create initial state
        let mut old_state = SessionState::new(ObjectId::new_v4(), "test-owner".to_string());
        old_state.id = session_id;

        // Create new state with changes
        let mut new_state = old_state.clone();

        // Add an object
        let obj_id = ObjectId::new_v4();
        new_state.objects.insert(
            obj_id,
            CADObject {
                id: obj_id,
                name: "Box1".to_string(),
                mesh: Mesh::new(),
                transform: Transform3D::identity(),
                material: shared_types::geometry::MaterialProperties {
                    diffuse_color: [0.5, 0.5, 0.5, 1.0],
                    metallic: 0.0,
                    roughness: 0.5,
                    emission: [0.0, 0.0, 0.0],
                    name: "default".to_string(),
                },
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                metadata: HashMap::new(),
                created_at: 0,
                modified_at: 0,
            },
        );

        // Add a user
        new_state
            .active_users
            .push(UserInfo::new("user1".to_string(), "User 1".to_string()));

        // Compute delta
        let delta = tracker.compute_delta(&old_state, &new_state).unwrap();

        assert_eq!(delta.object_deltas.len(), 1);
        match delta.object_deltas[0].delta_type {
            DeltaType::Added => (),
            _ => panic!("Expected Added delta type"),
        }
        assert!(delta.user_changes.is_some());
        assert_eq!(delta.user_changes.as_ref().unwrap().joined.len(), 1);
    }

    #[test]
    fn test_apply_delta() {
        let session_id = Uuid::new_v4();
        let mut tracker = DeltaTracker::new(session_id);

        let mut state = SessionState::new(ObjectId::new_v4(), "test-owner".to_string());
        state.id = session_id;

        // Create delta
        let obj_id = ObjectId::new_v4();
        let delta = SessionDelta {
            session_id,
            sequence: 1,
            timestamp: 1000,
            object_deltas: vec![ObjectDelta {
                id: obj_id,
                delta_type: DeltaType::Added,
                changes: None,
                object: Some(CADObject {
                    id: obj_id,
                    name: "Box1".to_string(),
                    mesh: Mesh::new(),
                    analytical_geometry: None,
                    cached_meshes: HashMap::new(),
                    transform: Transform3D::identity(),
                    material: shared_types::geometry::MaterialProperties {
                        diffuse_color: [0.5, 0.5, 0.5, 1.0],
                        metallic: 0.0,
                        roughness: 0.5,
                        emission: [0.0, 0.0, 0.0],
                        name: "default".to_string(),
                    },
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    metadata: HashMap::new(),
                    created_at: 0,
                    modified_at: 0,
                }),
                transform: None,
            }],
            timeline_delta: None,
            metadata_changes: None,
            user_changes: Some(UserChanges {
                joined: vec![UserInfo::new("user1".to_string(), "User 1".to_string())],
                left: vec![],
                updates: HashMap::new(),
            }),
            settings_changes: None,
        };

        // Apply delta
        tracker.apply_delta(&mut state, &delta).unwrap();

        assert_eq!(state.objects.len(), 1);
        assert!(state.objects.contains_key(&obj_id));
        assert_eq!(state.active_users.len(), 1);
        assert_eq!(state.active_users[0].id, "user1");
        assert_eq!(state.modified_at, 1000);
    }

    #[test]
    fn test_batch_deltas() {
        let session_id = Uuid::new_v4();
        let obj_id1 = ObjectId::new_v4();
        let obj_id2 = ObjectId::new_v4();

        let delta1 = SessionDelta {
            session_id,
            sequence: 1,
            timestamp: 1000,
            object_deltas: vec![ObjectDelta {
                id: obj_id1,
                delta_type: DeltaType::Added,
                changes: None,
                object: Some(CADObject {
                    id: obj_id1,
                    name: "Box1".to_string(),
                    mesh: Mesh::new(),
                    analytical_geometry: None,
                    cached_meshes: HashMap::new(),
                    transform: Transform3D::identity(),
                    material: shared_types::geometry::MaterialProperties {
                        diffuse_color: [0.5, 0.5, 0.5, 1.0],
                        metallic: 0.0,
                        roughness: 0.5,
                        emission: [0.0, 0.0, 0.0],
                        name: "default".to_string(),
                    },
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    metadata: HashMap::new(),
                    created_at: 0,
                    modified_at: 0,
                }),
                transform: None,
            }],
            timeline_delta: None,
            metadata_changes: None,
            user_changes: None,
            settings_changes: None,
        };

        let delta2 = SessionDelta {
            session_id,
            sequence: 2,
            timestamp: 2000,
            object_deltas: vec![ObjectDelta {
                id: obj_id2,
                delta_type: DeltaType::Added,
                changes: None,
                object: Some(CADObject {
                    id: obj_id2,
                    name: "Box2".to_string(),
                    mesh: Mesh::new(),
                    analytical_geometry: None,
                    cached_meshes: HashMap::new(),
                    transform: Transform3D::identity(),
                    material: shared_types::geometry::MaterialProperties {
                        diffuse_color: [0.5, 0.5, 0.5, 1.0],
                        metallic: 0.0,
                        roughness: 0.5,
                        emission: [0.0, 0.0, 0.0],
                        name: "default".to_string(),
                    },
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    metadata: HashMap::new(),
                    created_at: 0,
                    modified_at: 0,
                }),
                transform: None,
            }],
            timeline_delta: None,
            metadata_changes: None,
            user_changes: Some(UserChanges {
                joined: vec![UserInfo::new("user1".to_string(), "User 1".to_string())],
                left: vec![],
                updates: HashMap::new(),
            }),
            settings_changes: None,
        };

        let batched = batch_deltas(vec![delta1, delta2]).unwrap();

        assert_eq!(batched.sequence, 2);
        assert_eq!(batched.timestamp, 2000);
        assert_eq!(batched.object_deltas.len(), 2);
        assert!(batched.user_changes.is_some());
    }
}
