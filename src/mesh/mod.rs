//! Mesh layer - latency probing and shortest-path routing.
//!
//! Continuously probes RTT between all PoP pairs, smooths measurements
//! with EWMA, and runs Dijkstra to find the fastest path for each
//! destination.

pub mod latency_matrix;
pub mod probe;
pub mod router;
