mod types;
mod gpu;
mod sessions;

use crate::gpu::detect_nvidia_gpu;
use crate::sessions::{SessionStore, start_session, stop_session, list_sessions};
use crate::types::{HealthResponse, RegisterRequest, ResourceReport, HeartBeatRequest};

use axum::{extract::State, routing::{get, post}, Json, Router};
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};
use sysinfo::{Disks, System};
use tracing::info;
use tokio::sync::RwLock;


#[derive(Clone)]
struct AppState {
    disk_path: PathBuf,
}

fn build_agent(state: AppState, session_store: SessionStore) -> Router {
    let sessions_api = Router::new()
        .route("/v1/sessions/start", post(start_session))
        .route("/v1/sessions/stop", post(stop_session))
        .route("/v1/sessions", get(list_sessions))
        .with_state(session_store);

    let app = Router::new()
        .route("/ping", get(ping))
        .route("/health", get(health))
        .route("/v1/resources", get(resources))
        .merge(sessions_api)
        .with_state(state);

    return app;
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

    let session_store = SessionStore {
        sessions: Arc::new(RwLock::new(HashMap::new())),
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

    let agent = build_agent(state, session_store);

    let addr: SocketAddr = format!("{}:{}", host, port).parse().unwrap();
    info!("agent listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, agent).await.unwrap();
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

