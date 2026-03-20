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

//! Edge layer - user-facing connection acceptors.
//!
//! Accepts TCP and QUIC connections from end-users, ACKs locally for
//! minimal perceived latency, then relays traffic through the encrypted
//! tunnel mesh to the destination PoP.

pub mod quic_acceptor;
pub mod tcp_split;
