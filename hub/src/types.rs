use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GpuDevice {
    pub name: String,
    pub memory_total_mib: u64,
    pub memory_used_mib: u64,
    pub memory_free_mib: u64,
    pub utilization_gpu_pct: u32,
    pub temperature_c: Option<u32>,
    pub power_draw_w: Option<f32>,
    pub power_limit_w: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GpuInfo {
    pub kind: String,
    pub count: u32,
    pub driver_version: Option<String>,
    pub cuda_version: Option<String>,
    pub gpus: Vec<GpuDevice>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResourceReport {
    pub ram_total_mb: u64,
    pub ram_free_mb: u64,
    pub cpu_cores: u64,
    pub disk_free_mb: u64,
    pub disk_total_mb: u64,
    pub disk_path: String,
    pub gpu: Option<GpuInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeStatus {
    Up,
    Down,
}

#[derive(Debug, Clone)]
pub struct NodeState {
    pub id: String,
    pub url: String,
    pub status: NodeStatus,
    pub last_seen: DateTime<Utc>,
    pub last_error: Option<String>,
    pub resources: Option<ResourceReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeView {
    pub id: String,
    pub url: String,
    pub status: NodeStatus,
    pub last_seen: DateTime<Utc>,
    pub last_error: Option<String>,
    pub resources: Option<ResourceReport>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub image: String,
    pub requires_gpu: Option<bool>,
    pub cmd: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub node_id: String,
    pub agent_url: String,
    pub container_id: String,
    pub image: String,
    pub requires_gpu: bool,
    pub created_at: DateTime<Utc>,
    pub running_for_secs: i64,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub agent_id: String,
    pub agent_url: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    pub agent_id: String,
    pub resources: Option<ResourceReport>,
}

#[derive(Debug, Serialize)]
pub struct AgentStartSessionRequest {
    pub image: String,
    pub requires_gpu: Option<bool>,
    pub cmd: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct AgentStartSessionResponse {
    pub session_id: String,
    pub container_id: String,
}

#[derive(Debug, Serialize)]
pub struct AgentStopReq<'a> {
    pub session_id: &'a str,
}
