pub mod socket_bridge;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct PhpResponse {
    pub id: Option<String>,
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl PhpResponse {
    #[allow(dead_code)]
    pub fn new_success(id: Option<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            id,
            success: true,
            data,
            error: None,
        }
    }
    
    #[allow(dead_code)]
    pub fn new_error(id: Option<String>, error: String) -> Self {
        Self {
            id,
            success: false,
            data: None,
            error: Some(error),
        }
    }
}
