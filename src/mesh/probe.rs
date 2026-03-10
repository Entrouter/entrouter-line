/// Continuous latency probing between all PoP pairs.
/// Sends PING packets through tunnels, measures RTT from PONG responses.
/// Updates the latency matrix used by the mesh router.
use super::latency_matrix::LatencyMatrix;
use crate::relay::tunnel::Tunnel;
use crate::relay::wire;

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use tokio::time::{Duration, interval};
use tracing::{debug, warn};

pub struct Prober {
    node_id: String,
    matrix: Arc<LatencyMatrix>,
    /// probe_id → (peer_node_id, send_time)
    pending: DashMap<u32, (String, Instant)>,
    next_probe_id: AtomicU32,
}

impl Prober {
    pub fn new(node_id: String, matrix: Arc<LatencyMatrix>) -> Self {
        Self {
            node_id,
            matrix,
            pending: DashMap::new(),
            next_probe_id: AtomicU32::new(1),
        }
    }

    /// Handle an incoming PONG packet — compute RTT and update latency matrix
    pub fn handle_pong(&self, _from_peer: &str, payload: &[u8]) {
        if payload.len() < 4 {
            return;
        }
        let probe_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);

        if let Some((_, (peer_id, send_time))) = self.pending.remove(&probe_id) {
            let rtt = send_time.elapsed();
            debug!(
                from = %peer_id,
                rtt_us = rtt.as_micros(),
                "probe RTT"
            );
            // Update both directions (RTT is symmetric for our purposes)
            self.matrix.update(&self.node_id, &peer_id, rtt);
            self.matrix.update(&peer_id, &self.node_id, rtt);
        }
    }

    /// Create a PING payload and record the pending probe
    pub fn create_ping(&self, peer_id: &str) -> Vec<u8> {
        let probe_id = self.next_probe_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .insert(probe_id, (peer_id.to_string(), Instant::now()));

        // Clean up stale pending probes (older than 10s)
        self.pending
            .retain(|_, (_, t)| t.elapsed() < Duration::from_secs(10));

        probe_id.to_le_bytes().to_vec()
    }

    /// Create a PONG payload by echoing the PING payload
    pub fn create_pong(ping_payload: &[u8]) -> Vec<u8> {
        ping_payload.to_vec()
    }

    /// Start probing a specific peer at regular intervals
    pub async fn probe_loop(
        self: Arc<Self>,
        peer_id: String,
        tunnel: Arc<Tunnel>,
        interval_ms: u64,
    ) {
        let mut ticker = interval(Duration::from_millis(interval_ms));
        loop {
            ticker.tick().await;
            let ping_payload = self.create_ping(&peer_id);
            if let Err(e) = tunnel.send(wire::PACKET_PING, &ping_payload).await {
                warn!(peer = %peer_id, "probe send failed: {e}");
            }
        }
    }

    pub fn matrix(&self) -> &Arc<LatencyMatrix> {
        &self.matrix
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}
