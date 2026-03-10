//! Relay layer - encrypted tunnel transport with FEC.
//!
//! Provides the core packet relay pipeline: wire framing, ChaCha20-Poly1305
//! encryption, Reed-Solomon forward error correction, and multi-hop
//! forwarding through the mesh.

pub mod crypto;
pub mod fec;
pub mod fec_codec;
pub mod forwarder;
pub mod tunnel;
pub mod wire;
