//! **entrouter-line** — zero-loss cross-region packet relay mesh.
//!
//! Builds encrypted UDP tunnels between PoP nodes, adds adaptive FEC to
//! absorb packet loss, and routes traffic over the lowest-latency path
//! using live Dijkstra on a continuously-probed latency matrix.
//!
//! # Architecture
//!
//! * [`edge`] — User-facing TCP and QUIC acceptors that locally ACK traffic
//!   and relay it through the mesh.
//! * [`relay`] — Encrypted tunnel transport with FEC, wire framing, and
//!   multi-hop forwarding.
//! * [`mesh`] — Latency probing, EWMA smoothing, and shortest-path routing.
//! * [`admin`] — Lightweight HTTP server for health checks and status.
//! * [`config`] — TOML configuration loading and validation.

pub mod admin;
pub mod config;
pub mod edge;
pub mod mesh;
pub mod relay;
