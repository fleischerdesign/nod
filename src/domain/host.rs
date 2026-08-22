use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostRole {
    Desktop,
    Notebook,
    Server,
    Unknown(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntity {
    pub name: String,
    pub target_host: String,
    pub target_user: String,
    pub target_port: u16,
    pub role: HostRole,
    pub is_local: bool,
    pub active_closure: Option<PathBuf>,
}

impl HostEntity {
    pub fn new(name: impl Into<String>, target_host: impl Into<String>, is_local: bool) -> Self {
        Self {
            name: name.into(),
            target_host: target_host.into(),
            target_user: "root".to_string(),
            target_port: 22,
            role: HostRole::Server,
            is_local,
            active_closure: None,
        }
    }
}
