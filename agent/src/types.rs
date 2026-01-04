use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct HealthResponse{
    pub status: &'static str,
}

#[derive(Debug, Serialize, Clone)]
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

#[derive(Debug, Serialize, Clone)]
pub struct GpuInfo {
    pub kind: String,
    pub count: u32,
    pub driver_version: Option<String>,
    pub cuda_version: Option<String>,
    pub gpus: Vec<GpuDevice>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ResourceReport{
    pub ram_total_mb: u64,
    pub ram_free_mb: u64,
    pub cpu_cores: u64,
    pub disk_free_mb: u64,
    pub disk_total_mb: u64,
    pub disk_path: String,
    pub gpu: Option<GpuInfo>,
}

#[derive(Debug, serde::Serialize)]
pub struct RegisterRequest{
    pub agent_id: String,
    pub agent_url: String,
}

#[derive(Debug, serde::Serialize)]
pub struct HeartBeatRequest{
    pub agent_id: String,
    pub resources: Option<ResourceReport>,
}

#[derive(Debug, Deserialize)]
pub struct StartSessionRequest {
    pub image: String,
    pub requires_gpu: Option<bool>,
     pub cmd: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct StartSessionResponse {
    pub session_id: String,
    pub container_id: String,
}

#[derive(Debug, Deserialize)]
pub struct StopSessionRequest {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct StopSessionResponse {
    pub stopped: bool,
}

#[derive(Debug, Serialize)]
pub struct SessionView {
    pub session_id: String,
    pub container_id: String,
    pub created_at: DateTime<Utc>,
    pub uptime_secs: i64,
}

#[derive(Debug, Serialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionView>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMsg {
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16}
}