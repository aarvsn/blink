//! Serde types shared between the daemon, the panel, and Docker payloads.

use serde::{Deserialize, Serialize};

/// Server state strings (must match what the panel expects).
#[allow(dead_code)]
pub mod states {
    pub const CREATING: &str = "creating";
    pub const INSTALLING: &str = "installing";
    pub const INSTALL_FAILED: &str = "install_failed";
    pub const INSTALLED: &str = "installed";
    pub const STARTING: &str = "starting";
    pub const RUNNING: &str = "running";
    pub const UPDATING: &str = "updating";
    pub const UPDATE_FAILED: &str = "update_failed";
    pub const STOPPING: &str = "stopping";
    pub const STOPPED: &str = "stopped";
    pub const ERRORED: &str = "errored";
    pub const DELETING: &str = "deleting";
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CargoProperties {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_delete: Option<bool>,
    #[serde(
        rename = "noDelete",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub no_delete_camel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_properties: Option<serde_json::Value>,
}

impl CargoProperties {
    pub fn is_readonly(&self) -> bool {
        self.readonly.unwrap_or(false)
    }
    pub fn is_hidden(&self) -> bool {
        self.hidden.unwrap_or(false)
    }
    pub fn is_no_delete(&self) -> bool {
        self.no_delete.unwrap_or(false) || self.no_delete_camel.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoFile {
    pub id: String,
    pub url: String,
    #[serde(rename = "targetPath")]
    pub target_path: String,
    #[serde(default)]
    pub properties: CargoProperties,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerVariable {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    #[serde(rename = "defaultValue", alias = "default_value")]
    pub default_value: String,
    #[serde(default)]
    pub rules: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFileEntry {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallConfig {
    #[serde(rename = "dockerImage")]
    pub docker_image: String,
    #[serde(default)]
    pub entrypoint: String,
    #[serde(default)]
    pub script: String,
}

/// Full server configuration fetched from the panel
/// (`GET {appUrl}/api/servers/:id/config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(rename = "dockerImage")]
    pub docker_image: String,
    #[serde(default)]
    pub variables: Vec<ServerVariable>,
    #[serde(rename = "startupCommand")]
    pub startup_command: String,
    #[serde(rename = "configFiles", default)]
    pub config_files: Vec<ConfigFileEntry>,
    #[serde(default)]
    pub install: Option<InstallConfig>,
    #[serde(default)]
    pub cargo: Option<Vec<CargoFile>>,
    #[serde(rename = "serverControl", default)]
    pub server_control: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allocation {
    #[serde(rename = "bindAddress")]
    pub bind_address: String,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct CreateServerRequest {
    #[serde(rename = "serverId")]
    pub server_id: String,
    #[serde(rename = "validationToken")]
    pub validation_token: String,
    pub name: String,
    #[serde(rename = "memoryLimit")]
    pub memory_limit: i64,
    #[serde(rename = "cpuLimit")]
    pub cpu_limit: i64,
    pub allocation: Allocation,
    #[serde(rename = "dockerImage", default)]
    pub docker_image: Option<String>,
    /// Accepted and applied when provided via PATCH.
    #[serde(rename = "startupCommand", default)]
    #[allow(dead_code)]
    pub startup_command: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateServerRequest {
    #[serde(rename = "serverId")]
    pub server_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "memoryLimit", default)]
    pub memory_limit: Option<i64>,
    #[serde(rename = "cpuLimit", default)]
    pub cpu_limit: Option<i64>,
    #[serde(default)]
    pub allocation: Option<Allocation>,
    #[serde(rename = "unitChanged", default)]
    pub unit_changed: bool,
    #[serde(rename = "dockerImage", default)]
    pub docker_image: Option<String>,
    /// Accepted and applied when provided via PATCH.
    #[serde(rename = "startupCommand", default)]
    #[allow(dead_code)]
    pub startup_command: Option<String>,
    /// Accepted for wire compatibility; behaviour is driven by the other fields.
    #[serde(rename = "startupCommandChanged", default)]
    #[allow(dead_code)]
    pub startup_command_changed: bool,
    /// Accepted for wire compatibility; behaviour is driven by the other fields.
    #[serde(rename = "dockerImageChanged", default)]
    #[allow(dead_code)]
    pub docker_image_changed: bool,
}

/// Response the panel returns from `GET /api/servers/:internalId/validate/:token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateResponse {
    pub validated: bool,
    #[serde(default)]
    pub server: Option<ValidatedServer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedServer {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "internalId")]
    pub internal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoFilesResponse {
    #[serde(rename = "cargoFiles", default)]
    pub cargo_files: Vec<CargoFile>,
}

/// A row in the local `servers` table.
#[derive(Debug, Clone, Serialize)]
pub struct ServerRow {
    pub id: String,
    pub docker_id: Option<String>,
    pub name: String,
    pub image: String,
    pub state: String,
    pub memory_limit: i64,
    pub cpu_limit: i64,
    pub variables: String,
    pub startup_command: String,
    pub install_script: String,
    pub allocation: String,
    pub config_files: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
}

/// WebSocket message envelope `{ event, data }`.
#[derive(Debug, Clone, Serialize)]
pub struct WsEvent {
    pub event: String,
    pub data: serde_json::Value,
}

impl WsEvent {
    pub fn new(event: &str, data: serde_json::Value) -> Self {
        WsEvent { event: event.to_string(), data }
    }

    pub fn to_message(&self) -> axum::extract::ws::Message {
        axum::extract::ws::Message::Text(
            serde_json::to_string(self).unwrap_or_default().into(),
        )
    }
}
