use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Generic envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct Response<T> {
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// System info
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SystemInfo {
    pub version: String,
    pub start_time: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    Active,
    Inactive,
    Backoff,
    Error,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServiceInfo {
    pub name: String,
    pub startup: crate::plan::Startup,
    pub current: ServiceStatus,
    pub current_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct ServicesAction {
    pub action: ServiceAction,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Autostart,
    Replan,
}

// ---------------------------------------------------------------------------
// Changes / Tasks (simplified — no full StateEngine)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeStatus {
    Doing,
    Done,
    Error,
    Hold,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ChangeInfo {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub status: ChangeStatus,
    pub spawn_time: DateTime<Utc>,
    pub ready_time: Option<DateTime<Utc>>,
    pub err: Option<String>,
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Up,
    Down,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CheckInfo {
    pub name: String,
    pub level: crate::plan::CheckLevel,
    pub status: CheckStatus,
    pub failures: u32,
    pub threshold: u32,
}

// ---------------------------------------------------------------------------
// Exec
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExecRequest {
    pub command: Vec<String>,
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub timeout: Option<String>,
    pub service_context: Option<String>,
}

// ---------------------------------------------------------------------------
// Notices
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeType {
    ChangeUpdate,
    CheckFailed,
    CheckRecovered,
    Custom,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Notice {
    pub id: String,
    pub r#type: NoticeType,
    pub key: String,
    pub first_occurred: DateTime<Utc>,
    pub last_occurred: DateTime<Utc>,
    pub occurrences: u64,
}
