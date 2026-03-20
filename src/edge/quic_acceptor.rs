// Copyright 2026 John A Keeney - Entrouter
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

use std::sync::Arc;
/// QUIC 0-RTT acceptor for returning users.
/// Uses Quinn with self-signed certs for development.
/// Each QUIC bidirectional stream maps to a relay flow.
use std::sync::atomic::{AtomicU32, Ordering};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::relay::forwarder::{Forwarder, LocalDelivery};

/// QUIC 0-RTT edge acceptor.
/// Maps each QUIC bidirectional stream to a relay flow,
/// enabling fast reconnection for returning users.
pub struct QuicAcceptor {
    forwarder: Arc<Forwarder>,
    dest_node: String,
    active_flows: DashMap<u32, mpsc::Sender<Vec<u8>>>,
    next_flow_id: AtomicU32,
}

impl QuicAcceptor {
    /// Create a new QUIC acceptor forwarding to `dest_node` via the given forwarder.
    pub fn new(forwarder: Arc<Forwarder>, dest_node: String) -> Self {
        Self {
            forwarder,
            dest_node,
            active_flows: DashMap::new(),
            next_flow_id: AtomicU32::new(1_000_000), // offset from TCP flow IDs
        }
    }

    /// Start accepting QUIC connections
    pub async fn listen(self: Arc<Self>, endpoint: quinn::Endpoint) {
        info!("QUIC edge listening");
        while let Some(incoming) = endpoint.accept().await {
            let this = Arc::clone(&self);
            tokio::spawn(async move {
                match incoming.await {
                    Ok(conn) => {
                        debug!(remote = %conn.remote_address(), "QUIC connection");
                        this.handle_connection(conn).await;
                    }
                    Err(e) => {
                        warn!("QUIC accept error: {e}");
                    }
                }
            });
        }
    }

    async fn handle_connection(self: Arc<Self>, conn: quinn::Connection) {
        loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    let this = Arc::clone(&self);
                    tokio::spawn(async move {
                        this.handle_stream(send, recv).await;
                    });
                }
                Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                Err(e) => {
                    debug!("QUIC stream error: {e}");
                    break;
                }
            }
        }
    }

    async fn handle_stream(
        self: Arc<Self>,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) {
        let flow_id = self.next_flow_id.fetch_add(1, Ordering::Relaxed);
        let (resp_tx, mut resp_rx) = mpsc::channel::<Vec<u8>>(256);
        self.active_flows.insert(flow_id, resp_tx);

        let fwd = Arc::clone(&self.forwarder);
        let dest = self.dest_node.clone();

        let read_task = tokio::spawn(async move {
            let mut buf = [0u8; 16384];
            loop {
                match recv.read(&mut buf).await {
                    Ok(Some(n)) => {
                        if let Err(e) = fwd.send_to_node(&dest, flow_id, &buf[..n]).await {
                            warn!(flow_id, "relay send failed: {e}");
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        debug!(flow_id, "QUIC read error: {e}");
                        break;
                    }
                }
            }
        });

        let write_task = tokio::spawn(async move {
            while let Some(data) = resp_rx.recv().await {
                if send.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        tokio::select! {
            _ = read_task => {},
            _ = write_task => {},
        }

        self.active_flows.remove(&flow_id);
        debug!(flow_id, "QUIC flow ended");
    }

    /// Deliver incoming response data to the correct QUIC stream.
    pub fn deliver(&self, flow_id: u32, data: Vec<u8>) {
        if let Some(sender) = self.active_flows.get(&flow_id)
            && let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(data)
        {
            warn!(flow_id, "QUIC deliver dropped: channel full");
        }
    }

    /// Process deliveries from the relay (runs in background).
    pub async fn delivery_loop(self: Arc<Self>, mut rx: mpsc::Receiver<LocalDelivery>) {
        while let Some(delivery) = rx.recv().await {
            self.deliver(delivery.flow_id, delivery.data);
        }
    }

    /// Number of currently active QUIC flows.
    pub fn active_flow_count(&self) -> usize {
        self.active_flows.len()
    }
}

/// Generate a self-signed TLS certificate for QUIC (development only)
pub fn generate_self_signed_cert() -> (
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
) {
    let key_pair = rcgen::KeyPair::generate().expect("keygen failed");
    let params =
        rcgen::CertificateParams::new(vec!["localhost".into()]).expect("cert params failed");
    let cert = params.self_signed(&key_pair).expect("self-sign failed");

    let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
    let key_der = rustls::pki_types::PrivateKeyDer::try_from(key_pair.serialize_der())
        .expect("key serialization failed");

    (vec![cert_der], key_der)
}

/// Create a Quinn server config with self-signed certificate and 0-RTT enabled
pub fn make_server_config() -> quinn::ServerConfig {
    let (certs, key) = generate_self_signed_cert();

    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("TLS config failed");
    tls_config.alpn_protocols = vec![b"entrouter".to_vec()];
    // Enable TLS 0-RTT for returning clients.  Replay attacks on 0-RTT data
    // are safe here because the tunnel layer encrypts every packet with a
    // unique per-sequence nonce - replayed TLS early-data would decrypt to
    // duplicate tunnel packets that the receiver naturally deduplicates.
    tls_config.max_early_data_size = u32::MAX;

    let quic_config =
        quinn::crypto::rustls::QuicServerConfig::try_from(tls_config).expect("QUIC config failed");
    quinn::ServerConfig::with_crypto(Arc::new(quic_config))
}
