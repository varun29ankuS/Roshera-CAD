// Test file to debug WSQueryType serialization issue
use serde::{Deserialize, Serialize};
// Import specific types to avoid conflicts
use shared_types::scene_state::ObjectType;
use shared_types::{ObjectId, Timestamp};

// Copy of ObjectFilter to test
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestObjectFilter {
    pub object_type: Option<ObjectType>,
    pub created_after: Option<Timestamp>,
    pub created_before: Option<Timestamp>,
    pub modified_after: Option<Timestamp>,
    pub modified_before: Option<Timestamp>,
    pub tags: Option<Vec<String>>,
}

// Copy of WSQueryType to test
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "query")]
pub enum TestWSQueryType {
    GetObject {
        object_id: ObjectId,
    },
    ListObjects {
        filter: Option<TestObjectFilter>,
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
        #[serde(rename = "search_query")]
        query: String,
        limit: Option<usize>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wsquery_serialization() {
        let query = TestWSQueryType::GetObject {
            object_id: uuid::Uuid::new_v4(),
        };

        let json = serde_json::to_string(&query).unwrap();
        let _deserialized: TestWSQueryType = serde_json::from_str(&json).unwrap();
    }
}
