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

/// Shared state for all admin endpoints.
pub struct AdminState {
    pub node_id: String,
    pub region: String,
    pub matrix: Arc<LatencyMatrix>,
    pub forwarder: Arc<Forwarder>,
    pub tcp_splitter: Arc<TcpSplitter>,
    pub quic_acceptor: Arc<QuicAcceptor>,
    pub admin_token: Option<String>,
}

/// Build the admin [`Router`] with health and status endpoints.
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::mesh::latency_matrix::LatencyMatrix;
    use crate::mesh::probe::Prober;
    use crate::mesh::router::MeshRouter;
    use crate::relay::fec::FecConfig;
    use crate::relay::forwarder::Forwarder;

    fn test_state(token: Option<&str>) -> Arc<AdminState> {
        let matrix = Arc::new(LatencyMatrix::new());
        let router = Arc::new(MeshRouter::new("test-01".into(), Arc::clone(&matrix)));
        let prober = Arc::new(Prober::new("test-01".into(), Arc::clone(&matrix)));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let fwd = Arc::new(Forwarder::new(
            "test-01".into(),
            router,
            prober,
            tx,
            FecConfig { data_shards: 10, parity_shards: 4 },
        ));
        let tcp = Arc::new(TcpSplitter::new(Arc::clone(&fwd), "peer".into()));
        let quic = Arc::new(QuicAcceptor::new(Arc::clone(&fwd), "peer".into()));

        Arc::new(AdminState {
            node_id: "test-01".into(),
            region: "test".into(),
            matrix,
            forwarder: fwd,
            tcp_splitter: tcp,
            quic_acceptor: quic,
            admin_token: token.map(String::from),
        })
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = admin_router(test_state(None));
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn status_without_token_is_public() {
        let app = admin_router(test_state(None));
        let req = Request::get("/status").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn status_rejects_missing_token() {
        let app = admin_router(test_state(Some("secret")));
        let req = Request::get("/status").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn status_rejects_wrong_token() {
        let app = admin_router(test_state(Some("secret")));
        let req = Request::get("/status")
            .header("authorization", "Bearer wrong")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn status_accepts_correct_token() {
        let app = admin_router(test_state(Some("secret")));
        let req = Request::get("/status")
            .header("authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn status_body_contains_node_id() {
        let app = admin_router(test_state(None));
        let req = Request::get("/status").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["node_id"], "test-01");
        assert_eq!(json["region"], "test");
        assert_eq!(json["peers"], 0);
    }
}
