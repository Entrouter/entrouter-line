use base64::Engine;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::Path;

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
        Ok(())
    }
}

impl PeerConfig {
    /// Decode the shared key from base64.
    /// Returns an error if the key is invalid — callers should validate config first.
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
