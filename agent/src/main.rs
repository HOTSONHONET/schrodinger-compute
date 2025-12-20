use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use std::{net::SocketAddr, path::PathBuf};
use sysinfo::{Disks, System};
use tracing::info;


#[derive(Debug, Serialize)]
struct HealthResponse{
    status: &'static str,
}

#[derive(Debug, Serialize, Clone)]
struct GpuDevice {
    name: String,
    memory_total_mib: u64,
    memory_used_mib: u64,
    memory_free_mib: u64,
    utilization_gpu_pct: u32,
    temperature_c: Option<u32>,
    power_draw_w: Option<f32>,
    power_limit_w: Option<f32>,
}

#[derive(Debug, Serialize, Clone)]
struct GpuInfo {
    kind: String,
    count: u32,
    driver_version: Option<String>,
    cuda_version: Option<String>,
    gpus: Vec<GpuDevice>,
}

#[derive(Debug, Serialize, Clone)]
struct ResourceReport{
    ram_total_mb: u64,
    ram_free_mb: u64,
    cpu_cores: u64,
    disk_free_mb: u64,
    disk_total_mb: u64,
    disk_path: String,
    gpu: Option<GpuInfo>,
}

#[derive(Debug, serde::Serialize)]
struct RegisterRequest{
    agent_id: String,
    agent_url: String,
}

#[derive(Debug, serde::Serialize)]
struct HeartBeatRequest{
    agent_id: String,
    resources: Option<ResourceReport>,
}

#[derive(Clone)]
struct AppState {
    disk_path: PathBuf
}

#[tokio::main]
async fn main(){
    tracing_subscriber::fmt().init();

    // Loading env variables
    dotenvy::dotenv().ok();

    // Loading Config
    let host = std::env::var("AGENT_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("AGENT_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(7001);
    let hub_url = std::env::var("HUB_URL").ok();
    let hub_api_key = std::env::var("HUB_API_KEY").ok();
    let agent_id = std::env::var("AGENT_ID").ok();
    let agent_public_url = std::env::var("AGENT_PUBLIC_URL").ok();

    let disk_path_raw  = std::env::var("AGENT_DISK_PATH").unwrap_or_else(|_| "/".to_string());

    let disk_path = PathBuf::from(disk_path_raw);

    // Ensure disk path exist
    if let Err(e) = std::fs::create_dir_all(&disk_path) {
        panic!("Failed to create the AGENT_DISK_PATH dir : {} | Error: {}", disk_path.display(), e);
    }

    // Convert relative disk path to from root path
    let disk_path = match disk_path.canonicalize() {
        Ok(p) => p,
        Err(e) => panic!("failed to canonicalize disk path: {} | Error: {}", disk_path.display(), e),
    };

    let state = AppState {
        disk_path: disk_path,
    };

    // Launching heartbeat and request registering
    if let (
        Some(hub_url), 
        Some(hub_api_key),
        Some(agent_id),
        Some(agent_public_url)
    ) = (hub_url, hub_api_key, agent_id, agent_public_url) {
        
        spawn_hub_register_and_heartbeat(
            hub_url,
            hub_api_key,
            agent_id,
            agent_public_url,
            state.clone(),
        );
    }else{
        tracing::warn!("HUB_URL/HUB_API_KEY/AGENT_ID/AGENT_PUBLIC_URL not set; skipping hub discovery");
    }

    let app = Router::new()
    .route("/ping", get(ping))
    .route("/health", get(health))
    .route("/v1/resources", get(resources))
    .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse().unwrap();
    info!("agent listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ping() -> &'static str {
    return "what's up";
}

async fn health() -> Json<HealthResponse> {
    return Json(HealthResponse { status: "ok" })
}

async fn resources(State(state): State<AppState>) -> Json<ResourceReport> {
    return Json(collect_resources(&state).await);
}

async fn collect_resources(state: &AppState) -> ResourceReport {
    // Collect system information
    let mut sys = System::new_all();
    sys.refresh_all();

    let ram_total_mb = kb_to_mb(sys.total_memory());
    let ram_free_mb = kb_to_mb(sys.available_memory());
    let cpu_cores = sys.cpus().len() as u64;

    let (disk_free_mb, disk_total_mb) = disk_stats_for_path(&state.disk_path);

    let gpu = detect_nvidia_gpu();

    return ResourceReport { 
        ram_total_mb, 
        ram_free_mb, 
        cpu_cores,
        disk_free_mb, 
        disk_total_mb, 
        disk_path: state.disk_path.display().to_string(),
        gpu: gpu
    };
}

fn detect_nvidia_gpu() -> Option<GpuInfo>{
    use std::process::Command;

    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,memory.used,memory.free,utilization.gpu,temperature.gpu,power.draw,power.limit,driver_version", 
            "--format=--format=csv,noheader,nounits"])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    
    let mut gpus: Vec<GpuDevice> = Vec::new();
    let mut driver_version: Option<String> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // name, mem_total, mem_used, mem_free, util, temp, pwr_draw, pwr_limit, driver
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() < 9 {
            continue;
        }

        let name = parts[0].to_string();
        let mem_total = parts[1].parse::<u64>().unwrap_or(0);
        let mem_used = parts[2].parse::<u64>().unwrap_or(0);
        let mem_free = parts[3].parse::<u64>().unwrap_or(0);
        let util = parts[4].parse::<u32>().unwrap_or(0);

        // Optional numeric fields can sometimes be "N/A"
        let temp = parts[5].parse::<u32>().ok();
        let pwr_draw = parts[6].parse::<f32>().ok();
        let pwr_limit = parts[7].parse::<f32>().ok();

        if driver_version.is_none() && !parts[8].is_empty() {
            driver_version = Some(parts[8].to_string());
        }

        gpus.push(GpuDevice {
            name,
            memory_total_mib: mem_total,
            memory_used_mib: mem_used,
            memory_free_mib: mem_free,
            utilization_gpu_pct: util,
            temperature_c: temp,
            power_draw_w: pwr_draw,
            power_limit_w: pwr_limit,
        });
    }

    if gpus.is_empty() {
        return None;
    }

    let cuda_version = Command::new("nvidia-smi")
        .output()
        .ok()
        .and_then(|o| {
            if !o.status.success() {
                return None;
            }

            let s = String::from_utf8_lossy(&o.stdout);

            s.lines()
                .find(|l| l.contains("CUDA Version"))
                .and_then(|l| l.split("CUDA Version:").nth(1))
                .map(|v| v.trim().split_whitespace().next().unwrap_or("").to_string())
                .filter(|v| !v.is_empty())
        });


        return Some(GpuInfo { 
            kind: "nvidia".to_string(), 
            count: gpus.len() as u32,  
            driver_version, 
            cuda_version,
            gpus,
        });
}

fn disk_stats_for_path(path: &PathBuf) -> (u64, u64) {
    let disks = Disks::new_with_refreshed_list();

    let mut best: Option<(usize, u64, u64)> = None;

    for d in disks.list() {
        let mount = d.mount_point();
        if path.starts_with(mount) {
            let prefix_len = mount.as_os_str().len();
            let avail_mb = bytes_to_mb(d.available_space());
            let total_mb = bytes_to_mb(d.total_space());

            match best {
                None => best = Some((prefix_len, avail_mb, total_mb)),
                Some((best_len, _, _)) if prefix_len > best_len => {
                    best = Some((prefix_len, avail_mb, total_mb))
                }

                _ => {}
            }
        }
    }

    if let Some((_len, avail, total)) = best {
        return (avail, total)
    }else {
        return (0, 0);
    }
}

fn kb_to_mb(kb: u64) -> u64 {
    return kb / (1 << 10);
}

fn bytes_to_mb(bytes: u64) -> u64 {
    return bytes / (1 << 20);
}

fn spawn_hub_register_and_heartbeat (
    hub_url: String,
    api_key: String,
    agent_id: String,
    agent_public_url: String,
    state: AppState
 ) {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("reqwest client");
        
        // Register client with retries
        let register_url = format!("{}/v1/agents/register", hub_url);
        loop {
            let resp = client
                .post(&register_url)
                .header("x-api-key", &api_key)
                .json(&RegisterRequest{
                    agent_id: agent_id.clone(),
                    agent_url: agent_public_url.clone(),
                })
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => {
                    tracing::info!("registered to hub: {} -> {}", agent_id, agent_public_url);
                    break;
                }
                Ok(r) => tracing::warn!("hub register failed: {}", r.status()),
                Err(e) => tracing::warn!("hub register error: {}", e),
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        let heartbeat_url = format!("{}/v1/agents/heartbeat", hub_url);
        loop {
            let report = collect_resources(&state).await;

            let resp = client
                .post(&heartbeat_url)
                .header("x-api-key", &api_key)
                .json(&HeartBeatRequest{
                    agent_id: agent_id.clone(),
                    resources: Some(report),
                })
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => tracing::debug!("heartbeat ok"),
                Ok(r) => tracing::warn!("heartbeat failed: {}", r.status()),
                Err(e) => tracing::warn!("heartbeat error: {}", e),
            }
            tracing::debug!("heartbeat sent: {}", agent_id);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}