use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: String,
    pub display_name: String,
    pub name_set: bool,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("user_id", &self.user_id)
            .field("display_name", &self.display_name)
            .field("name_set", &self.name_set)
            .finish()
    }
}

impl Identity {
    pub fn generate(hostname: &str) -> Self {
        Self {
            user_id: Uuid::new_v4().to_string(),
            display_name: hostname.to_string(),
            name_set: false,
        }
    }

    /// Returns a public view of this identity without sensitive fields.
    pub fn to_public(&self) -> PublicIdentity {
        PublicIdentity {
            user_id: self.user_id.clone(),
            display_name: self.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicIdentity {
    pub user_id: String,
    pub display_name: String,
}
