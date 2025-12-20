use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use std::{net::SocketAddr, path::PathBuf};
use sysinfo::{Disks, System};
use tracing::info;


#[derive(Debug, Serialize)]
struct HealthResponse{
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ResourceReport{
    ram_total_mb: u64,
    ram_free_mb: u64,
    cpu_cores: u64,
    disk_free_mb: u64,
    disk_total_mb: u64,
    disk_path: String,
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
    // Collect system information
    let mut sys = System::new_all();
    sys.refresh_all();

    let ram_total_mb = kb_to_mb(sys.total_memory());
    let ram_free_mb = kb_to_mb(sys.available_memory());
    let cpu_cores = sys.cpus().len() as u64;

    let (disk_free_mb, disk_total_mb) = disk_stats_for_path(&state.disk_path);

    return Json(ResourceReport { ram_total_mb, 
        ram_free_mb, 
        cpu_cores,
        disk_free_mb, 
        disk_total_mb, 
        disk_path: state.disk_path.display().to_string(),
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