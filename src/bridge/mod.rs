pub mod socket_bridge;
pub mod worker_manager;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct PhpResponse {
    pub id: Option<String>,
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl PhpResponse {
    pub fn new_success(id: Option<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            id,
            success: true,
            data,
            error: None,
        }
    }

    pub fn new_error(id: Option<String>, error: String) -> Self {
        Self {
            id,
            success: false,
            data: None,
            error: Some(error),
        }
    }
}
