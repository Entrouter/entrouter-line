// Copyright 2025 John A Keeney - Entrouter
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

//! Mesh layer - latency probing and shortest-path routing.
//!
//! Continuously probes RTT between all PoP pairs, smooths measurements
//! with EWMA, and runs Dijkstra to find the fastest path for each
//! destination.

pub mod latency_matrix;
pub mod probe;
pub mod router;
