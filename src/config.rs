use base64::Engine;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    /// This node's unique identifier
    pub node_id: String,
    /// Region label (e.g. "syd", "lon", "sgp")
    pub region: String,
    /// Network listener configuration
    pub listen: ListenConfig,
    /// Mesh routing configuration
    pub mesh: MeshConfig,
    /// Relay configuration
    pub relay: RelayConfig,
    /// Known peer PoP nodes
    pub peers: Vec<PeerConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ListenConfig {
    /// UDP address for tunnel relay traffic
    pub relay_addr: SocketAddr,
    /// TCP address for edge user-facing traffic
    pub tcp_addr: SocketAddr,
    /// QUIC address for edge user-facing traffic
    pub quic_addr: SocketAddr,
    /// Admin HTTP address
    pub admin_addr: SocketAddr,
    /// Optional bearer token for admin /status endpoint
    pub admin_token: Option<String>,
    /// Optional TLS certificate path (PEM) for the TCP edge.
    /// If omitted, TCP edge runs without TLS (plaintext).
    pub tls_cert_path: Option<PathBuf>,
    /// Optional TLS private key path (PEM) for the TCP edge.
    /// Required if tls_cert_path is set.
    pub tls_key_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct MeshConfig {
    /// Probe interval in milliseconds
    pub probe_interval_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct RelayConfig {
    /// Default destination node for edge traffic
    pub default_dest: String,
    /// FEC data shards (N). Defaults to 10.
    pub fec_data_shards: Option<usize>,
    /// FEC parity shards (K). Defaults to 4.
    pub fec_parity_shards: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct PeerConfig {
    pub node_id: String,
    pub region: String,
    pub addr: SocketAddr,
    /// Base64-encoded 32-byte shared key
    pub shared_key: String,
}

impl Config {
    /// Load configuration from a TOML file
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        let config: Config =
            toml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.node_id.is_empty() {
            return Err(ConfigError::Validation("node_id cannot be empty".into()));
        }
        if self.peers.is_empty() {
            return Err(ConfigError::Validation("at least one peer required".into()));
        }
        for peer in &self.peers {
            let key_bytes = base64::engine::general_purpose::STANDARD
                .decode(&peer.shared_key)
                .map_err(|_| {
                    ConfigError::Validation(format!(
                        "invalid base64 shared_key for peer {}",
                        peer.node_id
                    ))
                })?;
            if key_bytes.len() != 32 {
                return Err(ConfigError::Validation(format!(
                    "shared_key for peer {} must be 32 bytes (got {})",
                    peer.node_id,
                    key_bytes.len()
                )));
            }
        }
        // Validate TLS config: cert and key must both be present or both absent
        match (&self.listen.tls_cert_path, &self.listen.tls_key_path) {
            (Some(_), None) => {
                return Err(ConfigError::Validation(
                    "tls_cert_path set without tls_key_path".into(),
                ));
            }
            (None, Some(_)) => {
                return Err(ConfigError::Validation(
                    "tls_key_path set without tls_cert_path".into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

impl PeerConfig {
    /// Decode the shared key from base64.
    /// Returns an error if the key is invalid - callers should validate config first.
    pub fn decode_key(&self) -> Result<[u8; 32], ConfigError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.shared_key)
            .map_err(|_| {
                ConfigError::Validation(format!(
                    "invalid base64 shared_key for peer {}",
                    self.node_id
                ))
            })?;
        let mut key = [0u8; 32];
        if bytes.len() != 32 {
            return Err(ConfigError::Validation(format!(
                "shared_key for peer {} must be 32 bytes (got {})",
                self.node_id,
                bytes.len()
            )));
        }
        key.copy_from_slice(&bytes);
        Ok(key)
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(String),
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "config I/O error: {e}"),
            Self::Parse(e) => write!(f, "config parse error: {e}"),
            Self::Validation(e) => write!(f, "config validation: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn valid_key_b64() -> String {
        base64::engine::general_purpose::STANDARD.encode([0xABu8; 32])
    }

    fn minimal_config(overrides: impl FnOnce(&mut Config)) -> Config {
        let mut cfg = Config {
            node_id: "test-01".into(),
            region: "test".into(),
            listen: ListenConfig {
                relay_addr: "127.0.0.1:4433".parse().unwrap(),
                tcp_addr: "127.0.0.1:8443".parse().unwrap(),
                quic_addr: "127.0.0.1:4434".parse().unwrap(),
                admin_addr: "127.0.0.1:9090".parse().unwrap(),
                admin_token: None,
                tls_cert_path: None,
                tls_key_path: None,
            },
            mesh: MeshConfig {
                probe_interval_ms: 1000,
            },
            relay: RelayConfig {
                default_dest: "peer-01".into(),
                fec_data_shards: None,
                fec_parity_shards: None,
            },
            peers: vec![PeerConfig {
                node_id: "peer-01".into(),
                region: "remote".into(),
                addr: "1.2.3.4:4433".parse().unwrap(),
                shared_key: valid_key_b64(),
            }],
        };
        overrides(&mut cfg);
        cfg
    }

    #[test]
    fn valid_config_passes() {
        let cfg = minimal_config(|_| {});
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn empty_node_id_fails() {
        let cfg = minimal_config(|c| c.node_id = "".into());
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("node_id"));
    }

    #[test]
    fn no_peers_fails() {
        let cfg = minimal_config(|c| c.peers.clear());
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("peer"));
    }

    #[test]
    fn bad_base64_key_fails() {
        let cfg = minimal_config(|c| c.peers[0].shared_key = "not-valid-base64!!!".into());
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("base64"));
    }

    #[test]
    fn wrong_key_length_fails() {
        let short_key = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        let cfg = minimal_config(|c| c.peers[0].shared_key = short_key);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("32 bytes"));
    }

    #[test]
    fn tls_cert_without_key_fails() {
        let cfg = minimal_config(|c| {
            c.listen.tls_cert_path = Some(PathBuf::from("/tmp/cert.pem"));
        });
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("tls_cert_path"));
    }

    #[test]
    fn tls_key_without_cert_fails() {
        let cfg = minimal_config(|c| {
            c.listen.tls_key_path = Some(PathBuf::from("/tmp/key.pem"));
        });
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("tls_key_path"));
    }

    #[test]
    fn tls_both_present_passes() {
        let cfg = minimal_config(|c| {
            c.listen.tls_cert_path = Some(PathBuf::from("/tmp/cert.pem"));
            c.listen.tls_key_path = Some(PathBuf::from("/tmp/key.pem"));
        });
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn decode_key_roundtrip() {
        let peer = PeerConfig {
            node_id: "n".into(),
            region: "r".into(),
            addr: "1.2.3.4:5".parse().unwrap(),
            shared_key: valid_key_b64(),
        };
        let key = peer.decode_key().unwrap();
        assert_eq!(key, [0xABu8; 32]);
    }
}
