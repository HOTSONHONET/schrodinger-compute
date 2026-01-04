mod types;
mod gpu;
mod sessions;

use crate::gpu::detect_nvidia_gpu;
use crate::sessions::{SessionStore, start_session, stop_session, list_sessions};
use crate::types::*;

use axum::{
    extract::{
        Path,
        State,
        ws::{
            WebSocketUpgrade,
            WebSocket,
            Message,
        }
    }, 
    response::IntoResponse,
    routing::{get, post}, Json, Router,
    body::Bytes,
};
use portable_pty::{
    native_pty_system,
    CommandBuilder,
    PtySize,
};
use std::{
    collections::HashMap, 
    net::SocketAddr, 
    path::PathBuf, 
    sync::Arc, 
    io::{Read, Write},
};
use futures_util::{
    StreamExt,
    SinkExt,
};
use sysinfo::{Disks, System};
use tracing::info;
use tokio::sync::{RwLock, mpsc};

#[derive(Clone)]
struct AppState {
    disk_path: PathBuf,
}

fn build_agent(state: AppState, session_store: SessionStore) -> Router {
    let sessions_api = Router::new()
        .route("/v1/sessions/start", post(start_session))
        .route("/v1/sessions/stop", post(stop_session))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{id}/ws", get(ws_attach_terminal))
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


pub async fn ws_attach_terminal (
    ws: WebSocketUpgrade,
    State(store): State<SessionStore>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {

    // Find container id
    let container_id = {
        let sessions = store.sessions.read().await;
        match sessions.get(&session_id) {
            Some(info) => info.container_id.clone(),
            None => return (
                axum::http::StatusCode::NOT_FOUND,
                "unknown session id"
            ).into_response(),
        }
    };

    return ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_terminal_ws(socket, container_id).await {
            tracing::warn!("terminal ws ended: {:?}", e);
        }
    });
}

async fn handle_terminal_ws(mut socket: WebSocket, container_id:String) -> anyhow::Result<()> {
    // Create PTY
    let pty_system = native_pty_system();
    let mut pair = pty_system.openpty(PtySize {
        rows: 24, cols: 80, pixel_width: 0, pixel_height: 0,
    })?;

    // Spawn docker exec inside the PTY, so that it can behave like real terminal
    let mut cmd = CommandBuilder::new("docker");
    cmd.args([
        "exec", "-it", &container_id,
        "sh", "-lc",
        "export TERM=xterm-256color; exec sh"
    ]);

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave); // keeping only master on the server side

    // Pump: PTY -> Websocket
    let (mut ws_tx, mut ws_rx) = socket.split();

    // channels
    // PTY -> WS
    let (pty_out_tx, mut pty_out_rx) = mpsc::channel::<Vec<u8>>(64);

    // WS -> PTY
    let (pty_in_tx, mut pty_in_rx) = mpsc::channel::<Vec<u8>>(64);

    // PTY Reader (blocking) -> pty_out_tx
    let mut reader = pair.master.try_clone_reader()?;
    let pty_read_task = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if pty_out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                },
                Err(_) => break,
            }
        }
    });

    // PTY writer (blocking) <- pty_in_rx
    let mut writer = pair.master.take_writer()?;
    let pty_write_task = tokio::task::spawn_blocking(move || {
        while let Some(data) = pty_in_rx.blocking_recv() {
            if writer.write_all(&data).is_err() {
                break;
            }

            let _ = writer.flush();
        }
    });

    // Async task: pty_out_rx -> ws_tx
    let ws_write_task = tokio::spawn(async move {
        while let Some(chunk) = pty_out_rx.recv().await {
            if ws_tx.send(Message::Binary(chunk.into())).await.is_err(){
                break;
            }
        }
    });

    // Pump: Websocket -> PTY
    while let Some(next) = ws_rx.next().await {
        let msg = match next {
            Ok(m) => m,
            Err(_) => break,  
        };
        
        match msg {
            Message::Binary(data) => {
                // writing to blocking thread
                if pty_in_tx.send(data.to_vec()).await.is_err() {
                    break;
                }
            }

            Message::Text(text) => {
                if let Ok(ctrl) = serde_json::from_str::<ControlMsg>(&text) {
                    if let ControlMsg::Resize  {cols, rows} = ctrl {
                        let _ = pair.master.resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                }else{
                    let mut bytes = text.as_bytes().to_vec();
                    bytes.push(b'\n');
                    let _ = pty_in_tx.send(bytes).await;
                }
            }

            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleaning up mess
    let _ = child.kill();
    drop(pty_in_tx);
    ws_write_task.abort();
    pty_read_task.abort();
    let _ = pty_write_task.await;

    return Ok(());
}

