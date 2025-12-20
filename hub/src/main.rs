use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tracing::{info, warn};

const AUTH_HEADER: &str = "x-api-key";

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ResourceReport {
    ram_total_mb: u64,
    ram_free_mb: u64,
    cpu_cores: u64,
    disk_free_mb: u64,
    disk_total_mb: u64,
    disk_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum NodeStatus {
    Up,
    Down,
}

#[derive(Debug, Clone)]
struct NodeState {
    id: String,
    url: String,
    status: NodeStatus,
    last_seen: DateTime<Utc>,
    last_error: Option<String>,
    resources: Option<ResourceReport>,
}

#[derive(Debug, Clone, Serialize)]
struct NodeView {
    id: String,
    url: String,
    status: NodeStatus,
    last_seen: DateTime<Utc>,
    last_error: Option<String>,
    resources: Option<ResourceReport>,
}

#[derive(Clone)]
struct AppState {
    api_key: String,
    stale_secs: i64,
    nodes: Arc<RwLock<HashMap<String, NodeState>>>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    agent_id: String,
    agent_url: String,
}

#[derive(Debug, Deserialize)]
struct HeartbeatRequest {
    agent_id: String,
    resources: Option<ResourceReport>,
}

// validate api key
fn check_api_key(headers: &HeaderMap, expected: &str) -> Result<(), (StatusCode, String)> {
    let got = headers
        .get(AUTH_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if got != expected {
        return Err((StatusCode::UNAUTHORIZED, "invalid api key".to_string()));
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    dotenvy::dotenv().ok();

    let host = std::env::var("HUB_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("HUB_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8001);

    let api_key = std::env::var("HUB_API_KEY").unwrap_or_else(|_| "dev-secret".to_string());

    let stale_secs: i64 = std::env::var("HUB_STALE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let state = AppState {
        api_key,
        stale_secs,
        nodes: Arc::new(RwLock::new(HashMap::new())),
    };

    spawn_stale_marker(state.clone());

    let app = Router::new()
        .route("/ping", get(ping))
        .route("/health", get(health))
        .route("/v1/agents/register", post(register_agent))
        .route("/v1/agents/heartbeat", post(heartbeat))
        .route("/v1/nodes", get(list_nodes))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse().unwrap();
    info!("hub listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn spawn_stale_marker(state: AppState) {
    tokio::spawn(async move {
        loop {
            mark_stale_nodes(&state).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn mark_stale_nodes(state: &AppState) {
    let now = Utc::now();
    let mut nodes = state.nodes.write().await;

    for (id, n) in nodes.iter_mut() {
        let age = (now - n.last_seen).num_seconds();
        if age > state.stale_secs {
            if matches!(n.status, NodeStatus::Up) {
                warn!("node {} DOWN (stale heartbeat: {}s)", id, age);
            }
            n.status = NodeStatus::Down;
            n.last_error = Some(format!("stale heartbeat: {}s", age));
        }
    }
}

async fn ping() -> &'static str {
    return "ok";
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn register_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    check_api_key(&headers, &state.api_key)?;

    let mut nodes = state.nodes.write().await;
    let now = Utc::now();

    let existed = nodes.contains_key(&req.agent_id);

    // UPSERT (update if exists, insert if not)
    nodes.insert(
        req.agent_id.clone(),
        NodeState {
            id: req.agent_id.clone(),
            url: req.agent_url.clone(),
            status: NodeStatus::Up,
            last_seen: now,
            last_error: None,
            resources: None,
        },
    );

    if existed {
        info!("agent re-registered: {} -> {}", req.agent_id, req.agent_url);
    } else {
        info!("agent registered: {} -> {}", req.agent_id, req.agent_url);
    }

    Ok(StatusCode::OK)
}

async fn heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<HeartbeatRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    check_api_key(&headers, &state.api_key)?;

    let mut nodes = state.nodes.write().await;
    let n = nodes
        .get_mut(&req.agent_id)
        .ok_or((StatusCode::NOT_FOUND, "agent not registered".to_string()))?;

    n.last_seen = Utc::now();
    n.status = NodeStatus::Up;
    n.last_error = None;

    if let Some(r) = req.resources {
        n.resources = Some(r);
    }

    Ok(StatusCode::OK)
}

async fn list_nodes(State(state): State<AppState>) -> Json<Vec<NodeView>> {
    let nodes = state.nodes.read().await;

    let mut out: Vec<NodeView> = nodes
        .values()
        .map(|n| NodeView {
            id: n.id.clone(),
            url: n.url.clone(),
            status: n.status.clone(),
            last_seen: n.last_seen,
            last_error: n.last_error.clone(),
            resources: n.resources.clone(),
        })
        .collect();

    out.sort_by(|a, b| a.id.cmp(&b.id));
    Json(out)
}
