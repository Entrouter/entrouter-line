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

/// Continuous latency prober for all PoP peers.
/// Sends PING packets through tunnels, computes RTT from PONG replies,
/// and feeds measurements into the shared [`LatencyMatrix`].
pub struct Prober {
    node_id: String,
    matrix: Arc<LatencyMatrix>,
    /// probe_id → (peer_node_id, send_time)
    pending: DashMap<u32, (String, Instant)>,
    next_probe_id: AtomicU32,
}

impl Prober {
    /// Create a new prober for the given local node.
    pub fn new(node_id: String, matrix: Arc<LatencyMatrix>) -> Self {
        Self {
            node_id,
            matrix,
            pending: DashMap::new(),
            next_probe_id: AtomicU32::new(1),
        }
    }

    /// Handle an incoming PONG packet - compute RTT and update latency matrix
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

    /// Reference to the underlying latency matrix.
    pub fn matrix(&self) -> &Arc<LatencyMatrix> {
        &self.matrix
    }

    /// Number of probes awaiting a PONG reply.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_prober() -> Prober {
        Prober::new("local".into(), Arc::new(LatencyMatrix::new()))
    }

    #[test]
    fn create_ping_returns_4_bytes() {
        let p = make_prober();
        let payload = p.create_ping("remote");
        assert_eq!(payload.len(), 4);
    }

    #[test]
    fn ping_ids_increment() {
        let p = make_prober();
        let p1 = p.create_ping("a");
        let p2 = p.create_ping("b");
        let id1 = u32::from_le_bytes([p1[0], p1[1], p1[2], p1[3]]);
        let id2 = u32::from_le_bytes([p2[0], p2[1], p2[2], p2[3]]);
        assert_eq!(id2, id1 + 1);
    }

    #[test]
    fn create_pong_echoes_payload() {
        let ping = vec![1, 2, 3, 4];
        let pong = Prober::create_pong(&ping);
        assert_eq!(pong, ping);
    }

    #[test]
    fn handle_pong_updates_matrix() {
        let p = make_prober();
        let ping = p.create_ping("remote");
        // Simulate a short delay by immediately handling the pong
        p.handle_pong("remote", &ping);
        // Matrix should now have an entry
        let rtt = p.matrix.get_rtt("local", "remote");
        assert!(rtt.is_some());
        // Symmetric update
        let rtt_rev = p.matrix.get_rtt("remote", "local");
        assert!(rtt_rev.is_some());
    }

    #[test]
    fn handle_pong_unknown_probe_id_ignored() {
        let p = make_prober();
        // Send a pong with a probe_id we never issued
        p.handle_pong("remote", &999u32.to_le_bytes());
        assert!(p.matrix.get_rtt("local", "remote").is_none());
    }

    #[test]
    fn handle_pong_short_payload_ignored() {
        let p = make_prober();
        p.handle_pong("remote", &[1, 2, 3]); // only 3 bytes, need 4
        assert!(p.matrix.get_rtt("local", "remote").is_none());
    }

    #[test]
    fn pending_count_tracks_outstanding() {
        let p = make_prober();
        assert_eq!(p.pending_count(), 0);
        let _ping1 = p.create_ping("a");
        let _ping2 = p.create_ping("b");
        assert_eq!(p.pending_count(), 2);
        // Handle one pong
        p.handle_pong("a", &_ping1);
        assert_eq!(p.pending_count(), 1);
    }
}
