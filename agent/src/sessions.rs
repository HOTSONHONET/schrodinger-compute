use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Utc};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use crate::types::{
    ListSessionsResponse, SessionView, StartSessionRequest, StartSessionResponse, StopSessionRequest,
    StopSessionResponse,
};

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub container_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SessionStore {
    pub sessions: Arc<RwLock<HashMap<String, SessionInfo>>>,
}


pub async fn start_session(
    State(state): State<SessionStore>,
    Json(req): Json<StartSessionRequest>,
) -> Result<Json<StartSessionResponse>, (StatusCode, String)> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let requires_gpu = req.requires_gpu.unwrap_or(false);

    let name = format!("sc-{}", session_id);

    let mut args: Vec<String> = vec!["run".into(), "-d".into(), "--name".into(), name.clone()];

    // GPU
    if requires_gpu {
        args.push("--gpus".into());
        args.push("all".into());
    }

    let cmd = req.cmd.unwrap_or_else(|| vec!["sh".into(), "-lc".into(), "sleep 360000".into()]);

    args.push(req.image.clone());
    args.extend(cmd);

    let output = tokio::process::Command::new("docker")
        .args(&args)
        .output()
        .await
        .map_err(
            |e| (
                StatusCode::INTERNAL_SERVER_ERROR, 
                format!("failed to run docker: {}", e)
            )
        )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err((StatusCode::BAD_REQUEST, format!("docker run failed: {}", stderr)));
    }

    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // saving the mapping between session id and container id
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(
            session_id.clone(), 
            SessionInfo { container_id: container_id.clone(), created_at: Utc::now() });
    }

    return Ok(Json(StartSessionResponse {session_id, container_id}));
}


pub async fn stop_session(
    State(state): State<SessionStore>,
    Json(req): Json<StopSessionRequest>,
) -> Result<Json<StopSessionResponse>, (StatusCode, String)> {
    
    let info = {
        let mut sessions = state.sessions.write().await;
        sessions.remove(&req.session_id) // returning the container_id
    };

    let Some(info) = info else {
        return Err((StatusCode::NOT_FOUND, "unknown session_id".to_string()));
    };

    let output = tokio::process::Command::new("docker")
        .args(["rm", "-f", &info.container_id])
        .output()
        .await
        .map_err(
            |e| (
                StatusCode::INTERNAL_SERVER_ERROR, 
                format!("failed to stop docker: {}", e)
            )
        )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err((StatusCode::BAD_REQUEST, format!("docker rm -f failed: {}", stderr)));
    }

    return Ok(Json(StopSessionResponse{
        stopped: true,
    }));
}

pub async fn list_sessions(
    State(store): State<SessionStore>
) -> Json<ListSessionsResponse> {
    let now = Utc::now();
    let sessions = store.sessions.read().await;

    let mut out: Vec<SessionView> = sessions
        .iter()
        .map(|(sid, info)| SessionView {
            session_id: sid.clone(),
            container_id: info.container_id.clone(),
            created_at: info.created_at,
            uptime_secs: (now - info.created_at).num_seconds(),
        })
        .collect();

    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    return Json(ListSessionsResponse {sessions: out});
}
