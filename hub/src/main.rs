mod types;

use crate::types::*;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::Utc;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tracing::{info, warn};
use reqwest;
use axum::extract::Path; 
use tower_http::cors::CorsLayer;

const AUTH_HEADER: &str = "x-api-key";

#[derive(Clone)]
struct AppState {
    api_key: String,
    stale_secs: i64,
    nodes: Arc<RwLock<HashMap<String, NodeState>>>,
    sessions: Arc<RwLock<HashMap<String, SessionRecord>>>,
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
        sessions: Arc::new(RwLock::new(HashMap::new())),
    };

    spawn_stale_marker(state.clone());

    let app = Router::new()
        .route("/ping", get(ping))
        .route("/health", get(health))
        .route("/v1/agents/register", post(register_agent))
        .route("/v1/agents/heartbeat", post(heartbeat))
        .route("/v1/agents", get(list_agents))
        .route("/v1/sessions", post(create_session))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{id}", axum::routing::delete(delete_session))
        .with_state(state)
        .layer(CorsLayer::permissive());

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

async fn list_agents(State(state): State<AppState>) -> Json<Vec<NodeView>> {
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

async fn create_session(
    State(state):  State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateSessionRequest>
) -> Result<Json<SessionRecord>, (StatusCode, String)> {

    check_api_key(&headers, &state.api_key)?;

    let requires_gpu = req.requires_gpu.unwrap_or(false);
    
    // picking nodes
    let nodes_guard = state.nodes.read().await;
    let node = pick_node(&nodes_guard, requires_gpu).ok_or((StatusCode::SERVICE_UNAVAILABLE, "no eligible nodes".to_string()))?;

    drop(nodes_guard);

    // call agent
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("client build: {}", e)))?;

    let url = format!("{}/v1/sessions/start", node.url);
    let agent_resp = client
        .post(&url)
        .json(&AgentStartSessionRequest{
            image: req.image.clone(),
            requires_gpu: Some(requires_gpu),
            cmd: req.cmd.clone(),
        })
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("agent unreachable: {}", e)))?;
    
    if !agent_resp.status().is_success() {
        let txt = agent_resp.text().await.unwrap_or_default();
        return Err((StatusCode::BAD_GATEWAY, format!("agent error: {}", txt)));
    }

    let started: AgentStartSessionResponse = agent_resp
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("bad agent json: {}", e)))?;

    let record = SessionRecord {
        session_id: started.session_id.clone(),
        node_id: node.id.clone(),
        agent_url: node.url.clone(),
        container_id: started.container_id.clone(),
        image: req.image.clone(),
        requires_gpu: requires_gpu,
        created_at: Utc::now(),
        running_for_secs: 0,
    };

    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(record.session_id.clone(), record.clone());
    }

    return Ok(Json(record));
}


async fn list_sessions(
    State(state): State<AppState>
) -> Json<Vec<SessionRecord>> {
    let sessions = state.sessions.read().await;
    let mut out: Vec<SessionRecord> = sessions.values().cloned().collect();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    return Json(out);
}

async fn delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {

    check_api_key(&headers, &state.api_key)?;   

    // find session
    let rec = {
        let sessions = state.sessions.read().await;
        sessions.get(&id).cloned().ok_or((StatusCode::NOT_FOUND, "unknown session_id".to_string()))?
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("client build: {}", e)))?;

    let stop_url = format!("{}/v1/sessions/stop", rec.agent_url);

    let resp = client
        .post(&stop_url)
        .json(&AgentStopReq {session_id: &id})
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("agent unreachable: {}", e)))?;

    if !resp.status().is_success() {
        let txt = resp.text().await.unwrap_or_default();
        return Err((StatusCode::BAD_GATEWAY, format!("agent stop failed: {}", txt)));
    }

    // Remove from session
    {
        let mut sessions = state.sessions.write().await;
        sessions.remove(&id);
    }

    return Ok(StatusCode::OK);

}

// Picking the node with highest available RAM
fn pick_node(nodes: &HashMap<String, NodeState>, requires_gpu: bool) -> Option<NodeState> {
    let mut candidates: Vec<&NodeState> = nodes.values()
        .filter(|n| matches!(n.status, NodeStatus::Up))
        .filter(|n| {
            if !requires_gpu {
                return true;
            }
            return n.resources.as_ref().and_then(|r| r.gpu.as_ref()).is_some();
        })
        .collect();

    candidates.sort_by_key(|n| {
        return n.resources.as_ref().map(|r| r.ram_free_mb).unwrap_or(0);
    });

    return candidates.last().cloned().cloned();
}