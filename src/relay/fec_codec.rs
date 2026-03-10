/// FEC codec - block-level Forward Error Correction for the relay tunnel.
///
/// Sits between the forwarder and tunnel to transparently add FEC protection.
///
/// Send path: accumulates relay payloads into blocks of `data_shards`,
/// generates `parity_shards` recovery shards, sends all via tunnel.
///
/// Recv path: collects shards by block_id, reconstructs blocks when
/// enough shards arrive, yields original relay payloads.
///
/// Shard header (7 bytes, prepended to each shard before encryption):
/// ```text
/// [2B block_id LE]     wrapping block sequence
/// [1B shard_idx]       0..total-1
/// [1B data_shards]     N
/// [1B parity_shards]   K
/// [2B original_len LE] actual payload length before padding
/// ```
use std::collections::HashMap;
use std::time::Instant;

use tracing::debug;

use crate::relay::fec::{FecConfig, FecEncoder};
use crate::relay::wire;

/// Size of the per-shard FEC header prepended before encryption (7 bytes).
pub const SHARD_HEADER_SIZE: usize = 7;

// --- Shard header encode/decode ---

fn encode_shard_header(
    buf: &mut [u8],
    block_id: u16,
    shard_idx: u8,
    data_shards: u8,
    parity_shards: u8,
    original_len: u16,
) {
    buf[0..2].copy_from_slice(&block_id.to_le_bytes());
    buf[2] = shard_idx;
    buf[3] = data_shards;
    buf[4] = parity_shards;
    buf[5..7].copy_from_slice(&original_len.to_le_bytes());
}

fn decode_shard_header(buf: &[u8]) -> Option<(u16, u8, u8, u8, u16)> {
    if buf.len() < SHARD_HEADER_SIZE {
        return None;
    }
    let block_id = u16::from_le_bytes([buf[0], buf[1]]);
    let shard_idx = buf[2];
    let data_shards = buf[3];
    let parity_shards = buf[4];
    let original_len = u16::from_le_bytes([buf[5], buf[6]]);
    Some((
        block_id,
        shard_idx,
        data_shards,
        parity_shards,
        original_len,
    ))
}

// --- FEC Sender ---

/// Accumulates relay payloads into FEC blocks, encodes parity, returns shards.
pub struct FecSender {
    config: FecConfig,
    block_id: u16,
    buffer: Vec<Vec<u8>>,
    payload_lens: Vec<u16>,
}

impl FecSender {
    /// Create a new FEC sender with the given shard configuration.
    pub fn new(config: FecConfig) -> Self {
        Self {
            config,
            block_id: 0,
            buffer: Vec::with_capacity(config.data_shards),
            payload_lens: Vec::with_capacity(config.data_shards),
        }
    }

    /// Submit a relay payload. Returns encoded shards if the block is full.
    /// Each shard is `(packet_type, payload_with_fec_header)`.
    pub fn submit(&mut self, payload: Vec<u8>) -> Option<Vec<(u8, Vec<u8>)>> {
        self.payload_lens.push(payload.len() as u16);
        self.buffer.push(payload);

        if self.buffer.len() >= self.config.data_shards {
            Some(self.flush())
        } else {
            None
        }
    }

    /// Flush whatever is buffered (for timeout). Returns None if empty.
    pub fn flush_partial(&mut self) -> Option<Vec<(u8, Vec<u8>)>> {
        if self.buffer.is_empty() {
            return None;
        }
        Some(self.flush())
    }

    /// Number of payloads buffered in the current (incomplete) block.
    pub fn buffered_count(&self) -> usize {
        self.buffer.len()
    }

    fn flush(&mut self) -> Vec<(u8, Vec<u8>)> {
        let block_id = self.block_id;
        self.block_id = self.block_id.wrapping_add(1);

        // Uniform shard size = max payload in this block
        // Prefix each payload with 2-byte length so it survives FEC reconstruction
        let mut shards: Vec<Vec<u8>> = self
            .buffer
            .drain(..)
            .map(|p| {
                let len = p.len() as u16;
                let mut v = Vec::with_capacity(2 + p.len());
                v.extend_from_slice(&len.to_le_bytes());
                v.extend_from_slice(&p);
                v
            })
            .collect();
        self.payload_lens.clear();

        // Uniform shard size = max prefixed payload
        let max_len = shards.iter().map(|s| s.len()).max().unwrap_or(2);

        // Pad all to uniform size
        for shard in &mut shards {
            shard.resize(max_len, 0);
        }

        // Pad remaining data slots with zeros if partial block
        while shards.len() < self.config.data_shards {
            shards.push(vec![0u8; max_len]);
        }

        // Generate parity shards
        let encoder = FecEncoder::new(self.config);
        encoder.encode(&mut shards);

        // Build output: FEC header + shard data
        let mut output = Vec::with_capacity(self.config.total_shards());
        for (i, shard) in shards.into_iter().enumerate() {
            let ptype = if i < self.config.data_shards {
                wire::PACKET_DATA
            } else {
                wire::PACKET_PARITY
            };

            // original_len in header is informational; real length is embedded in shard data
            let original_len = if i < self.config.data_shards && shard.len() >= 2 {
                u16::from_le_bytes([shard[0], shard[1]])
            } else {
                0
            };

            let mut framed = vec![0u8; SHARD_HEADER_SIZE + shard.len()];
            encode_shard_header(
                &mut framed,
                block_id,
                i as u8,
                self.config.data_shards as u8,
                self.config.parity_shards as u8,
                original_len,
            );
            framed[SHARD_HEADER_SIZE..].copy_from_slice(&shard);

            output.push((ptype, framed));
        }

        output
    }
}

// --- FEC Receiver ---

/// Partially-received FEC block being assembled.
struct PendingBlock {
    data_shards: usize,
    parity_shards: usize,
    shards: Vec<Option<Vec<u8>>>,
    received: usize,
    created_at: Instant,
}

impl PendingBlock {
    fn new(data_shards: u8, parity_shards: u8) -> Self {
        let total = data_shards as usize + parity_shards as usize;
        Self {
            data_shards: data_shards as usize,
            parity_shards: parity_shards as usize,
            shards: vec![None; total],
            received: 0,
            created_at: Instant::now(),
        }
    }

    /// Insert a shard. Returns true if we now have enough to reconstruct.
    fn insert(&mut self, shard_idx: u8, shard_data: Vec<u8>) -> bool {
        let idx = shard_idx as usize;
        let total = self.data_shards + self.parity_shards;
        if idx >= total || self.shards[idx].is_some() {
            return self.received >= self.data_shards;
        }

        self.shards[idx] = Some(shard_data);
        self.received += 1;

        self.received >= self.data_shards
    }

    /// Attempt reconstruction. Returns original data payloads on success.
    fn reconstruct(&mut self) -> Option<Vec<Vec<u8>>> {
        if self.received < self.data_shards {
            return None;
        }

        let config = FecConfig {
            data_shards: self.data_shards,
            parity_shards: self.parity_shards,
        };
        let encoder = FecEncoder::new(config);

        if encoder.reconstruct(&mut self.shards).is_ok() {
            let mut payloads = Vec::new();
            for i in 0..self.data_shards {
                if let Some(ref shard) = self.shards[i] {
                    if shard.len() < 2 {
                        continue;
                    }
                    // Length is embedded as first 2 bytes of the shard data
                    let len = u16::from_le_bytes([shard[0], shard[1]]) as usize;
                    if len == 0 {
                        continue; // padding shard
                    }
                    if 2 + len > shard.len() {
                        continue; // corrupted
                    }
                    payloads.push(shard[2..2 + len].to_vec());
                }
            }
            Some(payloads)
        } else {
            None
        }
    }
}

/// Collects incoming shards by block_id, reconstructs complete blocks.
#[derive(Default)]
pub struct FecReceiver {
    blocks: HashMap<u16, PendingBlock>,
}

impl FecReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process an incoming shard payload (FEC header + shard data).
    /// Returns reconstructed relay payloads when a block completes.
    pub fn receive_shard(&mut self, payload: &[u8]) -> Option<Vec<Vec<u8>>> {
        let (block_id, shard_idx, data_shards, parity_shards, _original_len) =
            decode_shard_header(payload)?;

        let shard_data = payload[SHARD_HEADER_SIZE..].to_vec();

        let block = self
            .blocks
            .entry(block_id)
            .or_insert_with(|| PendingBlock::new(data_shards, parity_shards));

        let can_reconstruct = block.insert(shard_idx, shard_data);

        if can_reconstruct && let Some(mut block) = self.blocks.remove(&block_id) {
            return block.reconstruct();
        }

        None
    }

    /// Expire incomplete blocks older than `max_age_ms` milliseconds.
    pub fn expire_old(&mut self, max_age_ms: u128) {
        let now = Instant::now();
        self.blocks.retain(|id, block| {
            let age = now.duration_since(block.created_at).as_millis();
            if age >= max_age_ms {
                debug!(
                    block_id = id,
                    received = block.received,
                    "expiring incomplete FEC block"
                );
            }
            age < max_age_ms
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_header_roundtrip() {
        let mut buf = [0u8; SHARD_HEADER_SIZE];
        encode_shard_header(&mut buf, 1234, 5, 10, 4, 800);
        let (bid, idx, ds, ps, len) = decode_shard_header(&buf).unwrap();
        assert_eq!(bid, 1234);
        assert_eq!(idx, 5);
        assert_eq!(ds, 10);
        assert_eq!(ps, 4);
        assert_eq!(len, 800);
    }

    #[test]
    fn sender_buffers_then_flushes() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let mut sender = FecSender::new(config);

        // First 3 submits should buffer
        assert!(sender.submit(vec![1u8; 100]).is_none());
        assert!(sender.submit(vec![2u8; 100]).is_none());
        assert!(sender.submit(vec![3u8; 100]).is_none());
        assert_eq!(sender.buffered_count(), 3);

        // 4th triggers flush: 4 data + 2 parity = 6 shards
        let shards = sender.submit(vec![4u8; 100]).unwrap();
        assert_eq!(shards.len(), 6);
        assert_eq!(sender.buffered_count(), 0);

        // Verify packet types
        assert_eq!(shards[0].0, wire::PACKET_DATA);
        assert_eq!(shards[3].0, wire::PACKET_DATA);
        assert_eq!(shards[4].0, wire::PACKET_PARITY);
        assert_eq!(shards[5].0, wire::PACKET_PARITY);
    }

    #[test]
    fn sender_partial_flush() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let mut sender = FecSender::new(config);

        sender.submit(vec![1u8; 50]);
        sender.submit(vec![2u8; 50]);
        assert_eq!(sender.buffered_count(), 2);

        // Partial flush sends what we have
        let shards = sender.flush_partial().unwrap();
        assert_eq!(shards.len(), 6); // still 4+2 (padded)
        assert_eq!(sender.buffered_count(), 0);
    }

    #[test]
    fn full_encode_decode_no_loss() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let mut sender = FecSender::new(config);
        let mut receiver = FecReceiver::new();

        let payloads: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8 + 10; 80]).collect();

        // Send all payloads
        let mut all_shards = Vec::new();
        for p in &payloads {
            if let Some(shards) = sender.submit(p.clone()) {
                all_shards = shards;
            }
        }
        assert_eq!(all_shards.len(), 6);

        // Deliver all shards to receiver
        let mut result = None;
        for (_ptype, shard) in &all_shards {
            if let Some(r) = receiver.receive_shard(shard) {
                result = Some(r);
            }
        }

        let recovered = result.unwrap();
        assert_eq!(recovered.len(), 4);
        for (i, p) in recovered.iter().enumerate() {
            assert_eq!(p, &payloads[i]);
        }
    }

    #[test]
    fn encode_decode_with_loss() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let mut sender = FecSender::new(config);
        let mut receiver = FecReceiver::new();

        let payloads: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8 + 20; 60]).collect();

        let mut all_shards = Vec::new();
        for p in &payloads {
            if let Some(shards) = sender.submit(p.clone()) {
                all_shards = shards;
            }
        }

        // Drop shards 0 and 2 (simulating network loss)
        let surviving: Vec<_> = all_shards
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 0 && *i != 2)
            .map(|(_, s)| s.clone())
            .collect();
        assert_eq!(surviving.len(), 4); // 6 - 2 = 4 (still >= data_shards)

        let mut result = None;
        for (_ptype, shard) in &surviving {
            if let Some(r) = receiver.receive_shard(shard) {
                result = Some(r);
            }
        }

        let recovered = result.unwrap();
        assert_eq!(recovered.len(), 4);
        for (i, p) in recovered.iter().enumerate() {
            assert_eq!(p, &payloads[i]);
        }
    }

    #[test]
    fn too_many_lost_returns_none() {
        let config = FecConfig {
            data_shards: 4,
            parity_shards: 2,
        };
        let mut sender = FecSender::new(config);
        let mut receiver = FecReceiver::new();

        let mut all_shards = Vec::new();
        for i in 0..4 {
            if let Some(shards) = sender.submit(vec![i as u8; 50]) {
                all_shards = shards;
            }
        }

        // Drop 3 shards - more than parity_shards(2), can't reconstruct
        let surviving: Vec<_> = all_shards.iter().skip(3).cloned().collect();
        assert_eq!(surviving.len(), 3);

        let mut result = None;
        for (_ptype, shard) in &surviving {
            if let Some(r) = receiver.receive_shard(shard) {
                result = Some(r);
            }
        }
        // Reconstruction attempted after receiving data_shards(4) but only 3 arrived
        // so it should never trigger
        assert!(result.is_none());
    }
}
