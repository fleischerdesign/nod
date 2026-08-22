use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredHostMeta {
    pub name: String,
    pub target_host: Option<String>,
    pub target_user: Option<String>,
    pub target_port: Option<u16>,
    pub role: Option<String>,
}
