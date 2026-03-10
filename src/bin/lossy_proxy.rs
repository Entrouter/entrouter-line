//! Lossy UDP proxy for testing FEC recovery.
//!
//! Sits between two tunnel endpoints and forwards packets while
//! randomly dropping a configurable percentage to simulate real-world loss.
//!
//! Usage:
//!   lossy-proxy --listen 127.0.0.1:9000 --target 127.0.0.1:8001 --loss 0.05

use clap::Parser;
use rand::Rng as _;
use rand::SeedableRng;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tracing::{debug, info};

#[derive(Parser)]
#[command(
    name = "lossy-proxy",
    about = "UDP proxy that drops packets to test FEC"
)]
struct Args {
    /// Address to listen on (receives from sender)
    #[arg(long, default_value = "127.0.0.1:9000")]
    listen: SocketAddr,

    /// Address to forward packets to (the receiver)
    #[arg(long, default_value = "127.0.0.1:8001")]
    target: SocketAddr,

    /// Drop rate as a fraction (0.0 = no drops, 0.05 = 5% drop)
    #[arg(long, default_value = "0.05")]
    loss: f64,

    /// Also proxy return traffic (bidirectional)
    #[arg(long, default_value = "false")]
    bidirectional: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    info!(
        listen = %args.listen,
        target = %args.target,
        loss = format!("{:.1}%", args.loss * 100.0),
        "lossy proxy starting"
    );

    let socket = UdpSocket::bind(args.listen)
        .await
        .expect("failed to bind proxy socket");

    let mut buf = [0u8; 2048];
    let mut rng = rand::rngs::StdRng::from_entropy();
    let mut total: u64 = 0;
    let mut dropped: u64 = 0;
    let mut sender_addr: Option<SocketAddr> = None;

    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("recv error: {e}");
                continue;
            }
        };

        total += 1;

        // Determine direction
        let is_from_target = from == args.target;

        if is_from_target && !args.bidirectional {
            // Forward return traffic without loss
            if let Some(addr) = sender_addr {
                let _ = socket.send_to(&buf[..len], addr).await;
            }
            continue;
        }

        if !is_from_target {
            sender_addr = Some(from);
        }

        // Apply random drop
        if rng.r#gen::<f64>() < args.loss {
            dropped += 1;
            debug!(total, dropped, "DROPPED packet");
            continue;
        }

        // Forward
        let dest = if is_from_target {
            sender_addr.unwrap_or(args.listen)
        } else {
            args.target
        };

        let _ = socket.send_to(&buf[..len], dest).await;

        if total.is_multiple_of(10000) {
            let actual_rate = dropped as f64 / total as f64;
            info!(
                total,
                dropped,
                actual_loss = format!("{:.2}%", actual_rate * 100.0),
                "proxy stats"
            );
        }
    }
}
