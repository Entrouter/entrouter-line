/// Admin HTTP endpoints — health, status, and metrics.
/// Lightweight axum server for monitoring and debugging.
/// If admin_token is configured, /status requires `Authorization: Bearer <token>`.
use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::edge::quic_acceptor::QuicAcceptor;
use crate::edge::tcp_split::TcpSplitter;
use crate::mesh::latency_matrix::LatencyMatrix;
use crate::relay::forwarder::Forwarder;

pub struct AdminState {
    pub node_id: String,
    pub region: String,
    pub matrix: Arc<LatencyMatrix>,
    pub forwarder: Arc<Forwarder>,
    pub tcp_splitter: Arc<TcpSplitter>,
    pub quic_acceptor: Arc<QuicAcceptor>,
    pub admin_token: Option<String>,
}

pub fn admin_router(state: Arc<AdminState>) -> Router {
    let health_route = Router::new().route("/health", get(health));

    let status_route = Router::new()
        .route("/status", get(status))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth_middleware,
        ));

    health_route
        .merge(status_route)
        .with_state(state)
}

async fn auth_middleware(
    State(state): State<Arc<AdminState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(expected) = &state.admin_token {
        let auth_header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok());

        match auth_header {
            Some(value) if value.starts_with("Bearer ") => {
                let token = &value[7..];
                if token != expected.as_str() {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
            _ => return Err(StatusCode::UNAUTHORIZED),
        }
    }

    Ok(next.run(req).await)
}

async fn health() -> &'static str {
    "ok"
}

async fn status(State(state): State<Arc<AdminState>>) -> Json<Value> {
    let edges = state.matrix.all_edges();
    let latencies: Vec<Value> = edges
        .iter()
        .map(|(from, to, rtt)| {
            json!({
                "from": from,
                "to": to,
                "rtt_us": rtt.as_micros(),
            })
        })
        .collect();

    Json(json!({
        "node_id": state.node_id,
        "region": state.region,
        "peers": state.forwarder.peer_count(),
        "tcp_flows": state.tcp_splitter.active_flow_count(),
        "quic_flows": state.quic_acceptor.active_flow_count(),
        "paths": state.matrix.path_count(),
        "latencies": latencies,
    }))
}
