use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use dashmap::DashMap;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use entrouter_line::admin;
use entrouter_line::config::Config;
use entrouter_line::edge::quic_acceptor::{self, QuicAcceptor};
use entrouter_line::edge::tcp_split::TcpSplitter;
use entrouter_line::mesh::latency_matrix::LatencyMatrix;
use entrouter_line::mesh::probe::Prober;
use entrouter_line::mesh::router::MeshRouter;
use entrouter_line::relay::crypto::TunnelCrypto;
use entrouter_line::relay::fec::FecConfig;
use entrouter_line::relay::forwarder::{Forwarder, LocalDelivery};
use entrouter_line::relay::tunnel::{self, ReceivedPacket, Tunnel};

/// Load a TLS server config from PEM cert + key files.
fn load_tls_config(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<rustls::ServerConfig, Box<dyn std::error::Error>> {
    let cert_data = std::fs::read(cert_path)?;
    let key_data = std::fs::read(key_path)?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_data[..]).collect::<Result<_, _>>()?;

    let key = rustls_pemfile::private_key(&mut &key_data[..])?
        .ok_or("no private key found in TLS key file")?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(config)
}

#[derive(Parser)]
#[command(name = "entrouter-line")]
#[command(about = "Zero-loss cross-region packet relay")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    if let Err(e) = run().await {
        error!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    info!("entrouter-line starting");

    // Load config
    let config = Config::load(&cli.config)?;

    info!(node = %config.node_id, region = %config.region, "config loaded");

    // --- Bind UDP socket for tunnel relay traffic (large buffers for burst absorption) ---
    let udp_socket = {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_recv_buffer_size(4 * 1024 * 1024).ok();
        sock.set_send_buffer_size(4 * 1024 * 1024).ok();
        sock.set_nonblocking(true)?;
        sock.bind(&config.listen.relay_addr.into())?;
        let std_sock: std::net::UdpSocket = sock.into();
        Arc::new(UdpSocket::from_std(std_sock)?)
    };
    info!(addr = %config.listen.relay_addr, "UDP relay bound (4MB buffers)");

    // --- Core components ---
    let matrix = Arc::new(LatencyMatrix::new());
    let router = Arc::new(MeshRouter::new(config.node_id.clone(), Arc::clone(&matrix)));
    let prober = Arc::new(Prober::new(config.node_id.clone(), Arc::clone(&matrix)));

    // Local delivery channel (forwarder → edge)
    let (local_tx, local_rx) = mpsc::channel::<LocalDelivery>(4096);

    let fec_config = FecConfig {
        data_shards: config.relay.fec_data_shards.unwrap_or(10),
        parity_shards: config.relay.fec_parity_shards.unwrap_or(4),
    };

    let forwarder = Arc::new(Forwarder::new(
        config.node_id.clone(),
        Arc::clone(&router),
        Arc::clone(&prober),
        local_tx,
        fec_config,
    ));

    // Forwarding event channel (receive loop → forwarder)
    let (fwd_tx, fwd_rx) = mpsc::channel::<(String, ReceivedPacket)>(8192);

    // --- Build peer map and create tunnels ---
    let peer_crypto_map: Arc<DashMap<std::net::SocketAddr, (String, TunnelCrypto)>> =
        Arc::new(DashMap::new());

    for peer in &config.peers {
        let key = peer.decode_key()?;

        // Tunnel for sending
        let tunnel = Arc::new(Tunnel::new(Arc::clone(&udp_socket), peer.addr, &key));
        forwarder.add_tunnel(peer.node_id.clone(), Arc::clone(&tunnel));

        // Register in peer crypto map for the multiplexed receive loop
        peer_crypto_map.insert(peer.addr, (peer.node_id.clone(), TunnelCrypto::new(&key)));

        // Start probe loop for this peer
        let prober_clone = Arc::clone(&prober);
        let tunnel_clone = Arc::clone(&tunnel);
        let probe_interval = config.mesh.probe_interval_ms;
        let peer_id = peer.node_id.clone();
        tokio::spawn(async move {
            prober_clone
                .probe_loop(peer_id, tunnel_clone, probe_interval)
                .await;
        });

        info!(peer = %peer.node_id, addr = %peer.addr, region = %peer.region, "tunnel ready");
    }

    // --- Start multiplexed receive loop (one loop for all peers) ---
    let recv_socket = Arc::clone(&udp_socket);
    let recv_tx = fwd_tx.clone();
    tokio::spawn(async move {
        tunnel::receive_loop_multi(recv_socket, peer_crypto_map, recv_tx).await;
    });
    drop(fwd_tx); // drop the extra sender so the forwarder loop can detect shutdown

    // --- Start forwarder ---
    let fwd_clone = Arc::clone(&forwarder);
    tokio::spawn(async move {
        fwd_clone.run(fwd_rx).await;
    });

    // --- TCP edge ---
    let tcp_listener = TcpListener::bind(config.listen.tcp_addr).await?;
    let mut tcp_splitter_inner =
        TcpSplitter::new(Arc::clone(&forwarder), config.relay.default_dest.clone());

    // Wrap with TLS if configured
    if let (Some(cert_path), Some(key_path)) =
        (&config.listen.tls_cert_path, &config.listen.tls_key_path)
    {
        let tls_config = load_tls_config(cert_path, key_path)?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
        tcp_splitter_inner = tcp_splitter_inner.with_tls(acceptor);
        info!(addr = %config.listen.tcp_addr, "TCP edge bound (TLS)");
    } else {
        info!(addr = %config.listen.tcp_addr, "TCP edge bound");
    }

    let tcp_splitter = Arc::new(tcp_splitter_inner);

    let tcp_clone = Arc::clone(&tcp_splitter);
    tokio::spawn(async move {
        tcp_clone.listen(tcp_listener).await;
    });

    // --- QUIC edge ---
    let quic_server_config = quic_acceptor::make_server_config();
    let quic_endpoint = quinn::Endpoint::server(quic_server_config, config.listen.quic_addr)?;
    let quic_acceptor = Arc::new(QuicAcceptor::new(
        Arc::clone(&forwarder),
        config.relay.default_dest.clone(),
    ));
    info!(addr = %config.listen.quic_addr, "QUIC edge bound");

    let quic_clone = Arc::clone(&quic_acceptor);
    tokio::spawn(async move {
        quic_clone.listen(quic_endpoint).await;
    });

    // --- Route local deliveries to edge (relay → TCP/QUIC clients) ---
    let tcp_delivery = Arc::clone(&tcp_splitter);
    let quic_delivery = Arc::clone(&quic_acceptor);
    tokio::spawn(async move {
        let mut local_rx = local_rx;
        while let Some(delivery) = local_rx.recv().await {
            // flow_id < 1_000_000 → TCP, >= 1_000_000 → QUIC
            if delivery.flow_id < 1_000_000 {
                tcp_delivery.deliver(delivery.flow_id, delivery.data);
            } else {
                quic_delivery.deliver(delivery.flow_id, delivery.data);
            }
        }
    });

    // --- Admin HTTP ---
    let admin_state = Arc::new(admin::AdminState {
        node_id: config.node_id.clone(),
        region: config.region.clone(),
        matrix: Arc::clone(&matrix),
        forwarder: Arc::clone(&forwarder),
        tcp_splitter: Arc::clone(&tcp_splitter),
        quic_acceptor: Arc::clone(&quic_acceptor),
        admin_token: config.listen.admin_token.clone(),
    });
    let admin_app = admin::admin_router(admin_state);
    let admin_listener = TcpListener::bind(config.listen.admin_addr).await?;
    info!(addr = %config.listen.admin_addr, "admin HTTP bound");

    tokio::spawn(async move {
        axum::serve(admin_listener, admin_app).await.ok();
    });

    // --- Ready ---
    info!(
        node = %config.node_id,
        region = %config.region,
        peers = config.peers.len(),
        "entrouter-line ready - all systems go"
    );

    tokio::signal::ctrl_c().await.ok();
    info!("shutting down");

    Ok(())
}
