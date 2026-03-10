//! Edge layer — user-facing connection acceptors.
//!
//! Accepts TCP and QUIC connections from end-users, ACKs locally for
//! minimal perceived latency, then relays traffic through the encrypted
//! tunnel mesh to the destination PoP.

pub mod quic_acceptor;
pub mod tcp_split;
