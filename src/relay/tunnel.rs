/// Pre-warmed encrypted UDP tunnel between PoPs.
/// Combines wire framing, ChaCha20-Poly1305 encryption, and adaptive FEC.
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{info, warn, debug};

use super::crypto::TunnelCrypto;
use super::fec::{FecConfig, FecEncoder, LossTracker};
use super::wire;

/// A bidirectional encrypted tunnel to a single peer.
pub struct Tunnel {
    pub peer_addr: SocketAddr,
    socket: Arc<UdpSocket>,
    crypto: TunnelCrypto,
    tx_seq: u16,
}

/// Received and decrypted payload from the tunnel.
pub struct ReceivedPacket {
    pub packet_type: u8,
    pub seq: u16,
    pub payload: Vec<u8>,
    pub from: SocketAddr,
}

impl Tunnel {
    pub fn new(socket: Arc<UdpSocket>, peer_addr: SocketAddr, key: &[u8; 32]) -> Self {
        Self {
            peer_addr,
            socket,
            crypto: TunnelCrypto::new(key),
            tx_seq: 0,
        }
    }

    /// Send a payload through the tunnel (encrypt + frame + UDP send).
    pub async fn send(&mut self, packet_type: u8, payload: &[u8]) -> std::io::Result<()> {
        let seq = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);

        // Encrypt payload
        let ciphertext = self.crypto.encrypt(seq, payload);
        let ct_len = ciphertext.len() as u16;

        // Build frame: header + ciphertext
        let mut frame = vec![0u8; wire::HEADER_SIZE + ciphertext.len()];
        wire::encode_header(&mut frame, packet_type, seq, ct_len);
        frame[wire::HEADER_SIZE..].copy_from_slice(&ciphertext);

        self.socket.send_to(&frame, self.peer_addr).await?;
        debug!(seq, peer = %self.peer_addr, "sent packet");
        Ok(())
    }

    /// Send a FEC-encoded block of data through the tunnel.
    /// Splits `data` into `data_shards` chunks, generates parity, sends all.
    pub async fn send_with_fec(&mut self, data: &[u8], fec_config: FecConfig) -> std::io::Result<()> {
        let encoder = FecEncoder::new(fec_config);
        let shard_size = (data.len() + fec_config.data_shards - 1) / fec_config.data_shards;

        // Split data into shards, pad last shard
        let mut shards: Vec<Vec<u8>> = data
            .chunks(shard_size)
            .map(|c| {
                let mut s = c.to_vec();
                s.resize(shard_size, 0);
                s
            })
            .collect();

        // If data doesn't fill all data_shards, pad with empty shards
        while shards.len() < fec_config.data_shards {
            shards.push(vec![0u8; shard_size]);
        }

        // Generate parity shards
        encoder.encode(&mut shards);

        // Send data shards
        for (i, shard) in shards.iter().enumerate() {
            let ptype = if i < fec_config.data_shards {
                wire::PACKET_DATA
            } else {
                wire::PACKET_PARITY
            };
            self.send(ptype, shard).await?;
        }

        Ok(())
    }
}

/// Receive loop — listens on the socket and decrypts incoming packets.
pub async fn receive_loop(
    socket: Arc<UdpSocket>,
    crypto: TunnelCrypto,
    tx: mpsc::Sender<ReceivedPacket>,
    mut loss_tracker: LossTracker,
) {
    let mut buf = [0u8; wire::MAX_PACKET];
    let mut expected_seq: u16 = 0;

    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                warn!("recv error: {e}");
                continue;
            }
        };

        if len < wire::HEADER_SIZE {
            continue;
        }

        let (packet_type, seq, payload_len) = wire::decode_header(&buf);
        let ct_start = wire::HEADER_SIZE;
        let ct_end = ct_start + payload_len as usize;

        if ct_end > len {
            warn!(seq, "truncated packet");
            loss_tracker.record(false);
            continue;
        }

        // Track gaps for loss measurement
        while expected_seq != seq {
            loss_tracker.record(false);
            expected_seq = expected_seq.wrapping_add(1);
        }
        loss_tracker.record(true);
        expected_seq = expected_seq.wrapping_add(1);

        let ciphertext = &buf[ct_start..ct_end];
        match crypto.decrypt(seq, ciphertext) {
            Ok(plaintext) => {
                let pkt = ReceivedPacket {
                    packet_type,
                    seq,
                    payload: plaintext,
                    from,
                };
                if tx.send(pkt).await.is_err() {
                    info!("receiver channel closed, stopping");
                    break;
                }
            }
            Err(e) => {
                warn!(seq, "decrypt failed: {e}");
                loss_tracker.record(false);
            }
        }

        // Log adaptive FEC recommendation periodically
        if seq % 1000 == 0 {
            let rate = loss_tracker.loss_rate();
            let rec = loss_tracker.recommended_config();
            debug!(
                loss_rate = format!("{:.2}%", rate * 100.0),
                data_shards = rec.data_shards,
                parity_shards = rec.parity_shards,
                "FEC recommendation"
            );
        }
    }
}

