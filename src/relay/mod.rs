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
