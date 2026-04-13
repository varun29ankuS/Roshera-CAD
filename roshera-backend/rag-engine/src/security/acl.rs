/// Access Control List implementation

use super::*;

pub struct ACLEntry {
    pub user_id: UserId,
    pub resource_id: DocumentId,
    pub permissions: Vec<Permission>,
}