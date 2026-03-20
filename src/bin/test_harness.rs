// Copyright 2025 John A Keeney - Entrouter
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Multi-node localhost test harness.
//!
//! Spawns two tunnel endpoints (sender + receiver) and an in-process
//! lossy proxy between them, then sends test data and verifies FEC recovery.
//!
//! Usage:
//!   test-harness [--loss 0.05] [--packets 1000] [--shard-size 1000]

use clap::Parser;
use rand::Rng as _;
use rand::SeedableRng;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

// Import from the library
use entrouter_line::relay::crypto::{self, TunnelCrypto};
use entrouter_line::relay::fec::{FecConfig, FecEncoder, LossTracker};
use entrouter_line::relay::wire;

#[derive(Parser)]
#[command(
    name = "test-harness",
    about = "End-to-end tunnel test with simulated loss"
)]
struct Args {
    /// Simulated packet loss rate (0.0 - 1.0)
    #[arg(long, default_value = "0.05")]
    loss: f64,

    /// Number of FEC blocks to send
    #[arg(long, default_value = "100")]
    blocks: usize,

    /// Size of each data block in bytes
    #[arg(long, default_value = "4000")]
    block_size: usize,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let loss_rate = args.loss;
    let num_blocks = args.blocks;
    let block_size = args.block_size;

    println!("=== Entrouter Line - End-to-End Test ===");
    println!("Loss rate: {:.1}%", loss_rate * 100.0);
    println!("Blocks: {num_blocks}");
    println!("Block size: {block_size} bytes");
    println!();

    // Shared encryption key
    let key = crypto::generate_key();
    let fec_config = FecConfig::for_loss_rate(loss_rate);
    println!(
        "FEC config: {} data + {} parity shards ({:.0}% overhead)",
        fec_config.data_shards,
        fec_config.parity_shards,
        fec_config.overhead() * 100.0
    );

    // Bind sender, proxy, receiver sockets
    let sender_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let proxy_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let receiver_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let sender_addr = sender_sock.local_addr().unwrap();
    let proxy_addr = proxy_sock.local_addr().unwrap();
    let receiver_addr = receiver_sock.local_addr().unwrap();

    println!("Sender:   {sender_addr}");
    println!("Proxy:    {proxy_addr}");
    println!("Receiver: {receiver_addr}");
    println!();

    let proxy_sock = Arc::new(proxy_sock);
    let receiver_sock = Arc::new(receiver_sock);

    // Channel for receiver results
    let (result_tx, mut result_rx) = mpsc::channel::<BlockResult>(num_blocks + 1);

    // --- Spawn lossy proxy task ---
    let proxy_sock_clone = proxy_sock.clone();
    let proxy_handle = tokio::spawn(async move {
        run_lossy_proxy(proxy_sock_clone, receiver_addr, loss_rate).await;
    });

    // --- Spawn receiver task ---
    let recv_key = key;
    let recv_sock = receiver_sock.clone();
    let receiver_handle = tokio::spawn(async move {
        run_receiver(recv_sock, recv_key, fec_config, num_blocks, result_tx).await;
    });

    // --- Sender: send FEC-encoded blocks ---
    let start = Instant::now();
    let crypto = TunnelCrypto::new(&key);
    let mut seq: u64 = 0;
    let mut rng = rand::thread_rng();

    for block_idx in 0..num_blocks {
        // Generate random test data
        let data: Vec<u8> = (0..block_size).map(|_| rng.r#gen()).collect();

        // Create FEC encoder
        let encoder = FecEncoder::new(fec_config);
        let shard_size = data.len().div_ceil(fec_config.data_shards);

        // Split into data shards
        let mut shards: Vec<Vec<u8>> = data
            .chunks(shard_size)
            .map(|c| {
                let mut s = c.to_vec();
                s.resize(shard_size, 0);
                s
            })
            .collect();
        while shards.len() < fec_config.data_shards {
            shards.push(vec![0u8; shard_size]);
        }

        // Encode FEC parity
        encoder.encode(&mut shards);

        // Send each shard as an encrypted packet through proxy
        for (i, shard) in shards.iter().enumerate() {
            let ptype = if i < fec_config.data_shards {
                wire::PACKET_DATA
            } else {
                wire::PACKET_PARITY
            };

            // Prefix shard with block_idx (4 bytes) + shard_idx (2 bytes) for reassembly
            let mut payload = Vec::with_capacity(6 + shard.len());
            payload.extend_from_slice(&(block_idx as u32).to_be_bytes());
            payload.extend_from_slice(&(i as u16).to_be_bytes());
            payload.extend_from_slice(shard);

            let ciphertext = crypto.encrypt(seq, &payload);
            let ct_len = ciphertext.len() as u16;

            let mut frame = vec![0u8; wire::HEADER_SIZE + ciphertext.len()];
            wire::encode_header(&mut frame, ptype, seq, ct_len);
            frame[wire::HEADER_SIZE..].copy_from_slice(&ciphertext);

            sender_sock.send_to(&frame, proxy_addr).await.unwrap();
            seq = seq.wrapping_add(1);
        }
    }

    // Send a termination signal
    let term_payload = b"DONE";
    let ciphertext = crypto.encrypt(seq, term_payload);
    let mut frame = vec![0u8; wire::HEADER_SIZE + ciphertext.len()];
    wire::encode_header(
        &mut frame,
        wire::PACKET_CONTROL,
        seq,
        ciphertext.len() as u16,
    );
    frame[wire::HEADER_SIZE..].copy_from_slice(&ciphertext);
    sender_sock.send_to(&frame, proxy_addr).await.unwrap();

    let send_elapsed = start.elapsed();

    // Wait for receiver to finish
    let _ = receiver_handle.await;
    proxy_handle.abort(); // Stop proxy

    let total_elapsed = start.elapsed();

    // Collect results
    println!("\n=== Results ===");
    let mut recovered = 0u64;
    let mut failed = 0u64;
    let mut total_shards_lost = 0u64;

    while let Ok(result) = result_rx.try_recv() {
        if result.success {
            recovered += 1;
        } else {
            failed += 1;
        }
        total_shards_lost += result.shards_lost as u64;
    }

    let total_data = num_blocks as u64 * block_size as u64;
    let total_shards_sent = num_blocks as u64 * fec_config.total_shards() as u64;

    println!("Blocks sent:       {num_blocks}");
    println!("Blocks recovered:  {recovered}");
    println!("Blocks failed:     {failed}");
    println!("Total shards sent: {total_shards_sent}");
    println!("Total shards lost: {total_shards_lost}");
    println!(
        "Actual loss rate:  {:.2}%",
        total_shards_lost as f64 / total_shards_sent as f64 * 100.0
    );
    println!("Data transferred:  {total_data} bytes");
    println!(
        "Send time:         {:.2}ms",
        send_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "Total time:        {:.2}ms",
        total_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "Throughput:        {:.2} MB/s",
        total_data as f64 / total_elapsed.as_secs_f64() / 1_000_000.0
    );

    if failed == 0 {
        println!(
            "\n*** ZERO-LOSS: All blocks recovered despite {:.1}% packet loss ***",
            loss_rate * 100.0
        );
    } else {
        println!("\n!!! {failed} blocks could NOT be recovered - FEC config may need tuning !!!");
    }
}

struct BlockResult {
    success: bool,
    shards_lost: usize,
}

/// In-process lossy proxy: forwards UDP packets with random drops.
async fn run_lossy_proxy(socket: Arc<UdpSocket>, target: SocketAddr, loss_rate: f64) {
    let mut buf = [0u8; 2048];
    let mut rng = rand::rngs::StdRng::from_entropy();
    let mut total: u64 = 0;
    let mut dropped: u64 = 0;
    let mut sender_addr: Option<SocketAddr> = None;

    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(_) => continue,
        };

        total += 1;

        if from == target {
            // Return traffic - forward without loss
            if let Some(addr) = sender_addr {
                let _ = socket.send_to(&buf[..len], addr).await;
            }
            continue;
        }

        sender_addr = Some(from);

        // Apply loss
        if rng.r#gen::<f64>() < loss_rate {
            dropped += 1;
            continue;
        }

        let _ = socket.send_to(&buf[..len], target).await;

        if total.is_multiple_of(5000) {
            tracing::debug!(
                total,
                dropped,
                actual_loss = format!("{:.2}%", dropped as f64 / total as f64 * 100.0),
                "proxy"
            );
        }
    }
}

/// Receiver: collect encrypted shards, decrypt, reassemble with FEC.
async fn run_receiver(
    socket: Arc<UdpSocket>,
    key: [u8; 32],
    fec_config: FecConfig,
    expected_blocks: usize,
    result_tx: mpsc::Sender<BlockResult>,
) {
    let crypto = TunnelCrypto::new(&key);
    let mut buf = [0u8; 2048];
    let mut loss_tracker = LossTracker::new(1000);
    let mut expected_seq: u64 = 0;

    // Collect shards per block: block_idx → (shard_idx → shard_data)
    let mut blocks: std::collections::HashMap<u32, Vec<Option<Vec<u8>>>> =
        std::collections::HashMap::new();

    let total_shards = fec_config.total_shards();

    loop {
        // Timeout after 2 seconds of no data (test complete)
        let recv = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            socket.recv_from(&mut buf),
        )
        .await;

        let (len, _from) = match recv {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => continue,
            Err(_) => break, // Timeout - assume done
        };

        if len < wire::HEADER_SIZE {
            continue;
        }

        let (packet_type, seq, payload_len) = wire::decode_header(&buf);
        let ct_end = wire::HEADER_SIZE + payload_len as usize;
        if ct_end > len {
            continue;
        }

        // Loss tracking
        while expected_seq != seq {
            loss_tracker.record(false);
            expected_seq = expected_seq.wrapping_add(1);
        }
        loss_tracker.record(true);
        expected_seq = expected_seq.wrapping_add(1);

        // Decrypt
        let ciphertext = &buf[wire::HEADER_SIZE..ct_end];
        let plaintext = match crypto.decrypt(seq, ciphertext) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Check for termination
        if packet_type == wire::PACKET_CONTROL && plaintext == b"DONE" {
            break;
        }

        if plaintext.len() < 6 {
            continue;
        }

        // Parse block_idx and shard_idx
        let block_idx =
            u32::from_be_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]);
        let shard_idx = u16::from_be_bytes([plaintext[4], plaintext[5]]) as usize;
        let shard_data = plaintext[6..].to_vec();

        let block = blocks
            .entry(block_idx)
            .or_insert_with(|| vec![None; total_shards]);
        if shard_idx < total_shards {
            block[shard_idx] = Some(shard_data);
        }
    }

    // Reconstruct all blocks
    for block_idx in 0..expected_blocks as u32 {
        let block = blocks
            .entry(block_idx)
            .or_insert_with(|| vec![None; total_shards]);
        let shards_lost = block.iter().filter(|s| s.is_none()).count();

        let encoder = FecEncoder::new(fec_config);
        let success = encoder.reconstruct(block).is_ok();

        let _ = result_tx
            .send(BlockResult {
                success,
                shards_lost,
            })
            .await;
    }

    let rate = loss_tracker.loss_rate();
    println!(
        "Receiver: measured loss rate = {:.2}%, recommended FEC = {}+{}",
        rate * 100.0,
        loss_tracker.recommended_config().data_shards,
        loss_tracker.recommended_config().parity_shards,
    );
}
