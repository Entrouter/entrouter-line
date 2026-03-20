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

//! **entrouter-line** - zero-loss cross-region packet relay mesh.
//!
//! Builds encrypted UDP tunnels between PoP nodes, adds adaptive FEC to
//! absorb packet loss, and routes traffic over the lowest-latency path
//! using live Dijkstra on a continuously-probed latency matrix.
//!
//! # Architecture
//!
//! * [`edge`] - User-facing TCP and QUIC acceptors that locally ACK traffic
//!   and relay it through the mesh.
//! * [`relay`] - Encrypted tunnel transport with FEC, wire framing, and
//!   multi-hop forwarding.
//! * [`mesh`] - Latency probing, EWMA smoothing, and shortest-path routing.
//! * [`admin`] - Lightweight HTTP server for health checks and status.
//! * [`config`] - TOML configuration loading and validation.

pub mod admin;
pub mod config;
pub mod edge;
pub mod mesh;
pub mod relay;
