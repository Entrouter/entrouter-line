use std::sync::Arc;
/// TCP connection splitter at the edge.
/// ACKs user immediately (low local RTT), buffers and relays over the fast tunnel.
/// Each TCP connection maps to a relay flow_id for end-to-end tracking.
use std::sync::atomic::{AtomicU32, Ordering};

use dashmap::DashMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

use crate::relay::forwarder::{Forwarder, LocalDelivery};

/// Max user data per relay payload to avoid IP fragmentation.
/// With relay header (~11B), FEC header (7B), length prefix (2B),
/// wire header (5B), and auth tag (16B), each shard stays under MTU.
const MAX_RELAY_DATA: usize = 1400;

/// TCP edge splitter.
/// Accepts user TCP connections, ACKs locally for low latency,
/// then relays traffic over the encrypted tunnel mesh.
pub struct TcpSplitter {
    forwarder: Arc<Forwarder>,
    dest_node: String,
    /// flow_id → sender to write response data back to the client
    active_flows: DashMap<u32, mpsc::Sender<Vec<u8>>>,
    next_flow_id: AtomicU32,
    /// When set, incoming TCP connections are upgraded to TLS before processing.
    tls_acceptor: Option<TlsAcceptor>,
}

impl TcpSplitter {
    /// Create a new TCP splitter forwarding to `dest_node` via the given forwarder.
    pub fn new(forwarder: Arc<Forwarder>, dest_node: String) -> Self {
        Self {
            forwarder,
            dest_node,
            active_flows: DashMap::new(),
            next_flow_id: AtomicU32::new(1),
            tls_acceptor: None,
        }
    }

    /// Enable TLS for incoming connections using the given acceptor.
    pub fn with_tls(mut self, acceptor: TlsAcceptor) -> Self {
        self.tls_acceptor = Some(acceptor);
        self
    }

    /// Start accepting TCP connections
    pub async fn listen(self: Arc<Self>, listener: TcpListener) {
        let tls_label = if self.tls_acceptor.is_some() { " (TLS)" } else { "" };
        let addr = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "unknown".into());
        info!(%addr, "TCP edge listening{tls_label}");
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!(client = %addr, "new TCP connection");
                    let this = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Some(ref acceptor) = this.tls_acceptor {
                            match acceptor.accept(stream).await {
                                Ok(tls_stream) => {
                                    this.handle_connection(tls_stream).await;
                                }
                                Err(e) => {
                                    warn!(client = %addr, "TLS handshake failed: {e}");
                                }
                            }
                        } else {
                            this.handle_connection(stream).await;
                        }
                    });
                }
                Err(e) => {
                    warn!("TCP accept error: {e}");
                }
            }
        }
    }

    async fn handle_connection<S>(self: Arc<Self>, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let flow_id = self.next_flow_id.fetch_add(1, Ordering::Relaxed);
        let (mut reader, mut writer) = tokio::io::split(stream);

        // Channel for response data coming back from the relay
        let (resp_tx, mut resp_rx) = mpsc::channel::<Vec<u8>>(2048);
        self.active_flows.insert(flow_id, resp_tx);

        let fwd = Arc::clone(&self.forwarder);
        let dest = self.dest_node.clone();

        // Task: read from client → chunk into MTU-safe payloads → send through relay
        let read_task = tokio::spawn(async move {
            let mut buf = [0u8; 16384];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        for chunk in buf[..n].chunks(MAX_RELAY_DATA) {
                            if let Err(e) = fwd.send_to_node(&dest, flow_id, chunk).await {
                                warn!(flow_id, "relay send failed: {e}");
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        debug!(flow_id, "client read error: {e}");
                        break;
                    }
                }
            }
        });

        // Task: receive relay responses → write to client
        let write_task = tokio::spawn(async move {
            while let Some(data) = resp_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        tokio::select! {
            _ = read_task => {},
            _ = write_task => {},
        }

        self.active_flows.remove(&flow_id);
        debug!(flow_id, "TCP flow ended");
    }

    /// Deliver incoming response data to the correct TCP client
    pub fn deliver(&self, flow_id: u32, data: Vec<u8>) {
        if let Some(sender) = self.active_flows.get(&flow_id)
            && let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(data)
        {
            warn!(flow_id, "TCP deliver dropped: channel full");
        }
    }

    /// Process deliveries from the relay (runs in background)
    pub async fn delivery_loop(self: Arc<Self>, mut rx: mpsc::Receiver<LocalDelivery>) {
        while let Some(delivery) = rx.recv().await {
            self.deliver(delivery.flow_id, delivery.data);
        }
    }

    /// Number of currently active TCP flows.
    pub fn active_flow_count(&self) -> usize {
        self.active_flows.len()
    }
}
