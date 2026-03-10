/// Admin HTTP endpoints — health, status, and metrics.
/// Lightweight axum server for monitoring and debugging.
use std::sync::Arc;

use axum::extract::State;
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
}

pub fn admin_router(state: Arc<AdminState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .with_state(state)
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
