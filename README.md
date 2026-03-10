# Entrouter Line

[![Build](https://github.com/Entrouter/entrouter-line/actions/workflows/ci.yml/badge.svg)](https://github.com/Entrouter/entrouter-line/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.94+-orange.svg)](https://www.rust-lang.org)

**Zero-loss cross-region packet relay network.**

Adaptive FEC, encrypted UDP tunnels, real-time latency-mesh routing, and QUIC 0-RTT edge termination. Written in Rust.

---

## What This Does

Relays packets between globally distributed PoP (Point of Presence) nodes with:

- **Zero packet loss up to 10% link loss** — adaptive Reed-Solomon FEC absorbs all loss with zero throughput impact
- **Zero relay overhead** — measured loss exactly matches simulated network loss, the relay adds nothing
- **Optimal routing** via real-time latency mesh with Dijkstra shortest-path (not BGP)
- **Instant connections** via QUIC 0-RTT + TCP splitting at edge
- **Always-encrypted** tunnels with ChaCha20-Poly1305

## Benchmarks

Tested on London ↔ Sydney (273ms RTT) over Vultr shared VPS — one of the longest internet routes on Earth, on budget infrastructure.

### FEC Loss Recovery

| Link Loss | Throughput vs Baseline | Status |
|-----------|----------------------|--------|
| 0% | 100% | Baseline |
| 5% | 100% | **Perfect recovery** |
| 10% | 99% | **Perfect recovery** |
| 20% | 87% | Matches theoretical prediction within 1% |
| 22% | 83% | Graceful degradation |
| 25%+ | FAIL | QUIC control plane limit (see below) |

### Relay Overhead

| Link Loss | Relay Added Loss |
|-----------|-----------------|
| 1% | **0%** |
| 5% | **0%** |
| 10% | **0%** |
| 20% | **0%** |

The relay introduces zero additional packet loss at any tested loss level. Encryption, header routing, and tunnel forwarding add no measurable overhead.

### Infrastructure Limits (Not Code Limits)

- **~140 Mbps throughput cap:** Vultr VPS NIC/bandwidth allocation, not the relay. On bare metal or higher-tier VPS, throughput scales with the NIC.
- **25%+ loss failure:** At 25% unidirectional loss over 273ms RTT, each QUIC round-trip faces ~44% compound loss. No QUIC implementation (Quinn, quiche, msquic) survives this. This is a physical link constraint, not a relay limitation. On shorter routes or better infrastructure, the operational ceiling is higher.
- **Real-world context:** Internet backbone loss between major cities is typically 0.01–2%. This relay handles that range with zero visible loss. Even 10–20% loss (damaged undersea cable territory) still delivers 87%+ throughput.

Full benchmark methodology and raw data: [BENCHMARK-RESULTS.md](BENCHMARK-RESULTS.md)

---

## Quick Start

### 1. Build

```bash
cargo build --release
```

### 2. Configure

Copy the example config and edit for your nodes:

```bash
cp config.example.toml config.toml
```

```toml
node_id = "us-east-01"
region = "us-east"

[listen]
relay_addr = "0.0.0.0:4433"
tcp_addr   = "0.0.0.0:8443"
quic_addr  = "0.0.0.0:4434"
admin_addr = "127.0.0.1:9090"

[[peers]]
node_id    = "eu-west-01"
region     = "eu-west"
addr       = "1.2.3.4:4433"
shared_key = "base64-encoded-32-byte-key"
```

Generate a shared key:
```bash
openssl rand -base64 32
```

### 3. Run

```bash
./target/release/entrouter-line --config config.toml
```

The admin API is available at `http://127.0.0.1:9090`:
- `GET /health` — liveness check
- `GET /status` — peer connections, latency matrix, routing table

---

## Architecture

```
User → [QUIC 0-RTT] → Edge PoP → [UDP + FEC + ChaCha20 Tunnel] → Edge PoP → Origin
```

## Project Structure

```
src/
├── main.rs              # Entry point
├── config.rs            # Node & peer configuration
├── relay/               # Core relay engine
│   ├── tunnel.rs        # Encrypted UDP tunnels
│   ├── fec.rs           # Adaptive Forward Error Correction
│   ├── forwarder.rs     # Packet forwarding & routing
│   ├── crypto.rs        # ChaCha20-Poly1305 encryption
│   └── wire.rs          # Binary wire protocol
├── mesh/                # Routing mesh
│   ├── probe.rs         # Latency probing
│   ├── router.rs        # Dijkstra shortest-path routing
│   └── latency_matrix.rs
└── edge/                # Edge termination
    ├── tcp_split.rs     # TCP connection splitting
    └── quic_acceptor.rs # QUIC 0-RTT acceptor
```

## Build

```bash
cargo build --release
```

## Contributing

Issues and pull requests welcome. If you're reporting a bug, include:
- Your config (redact keys)
- OS and Rust version
- Steps to reproduce
- Relevant logs

## License

MIT — see [LICENSE](LICENSE)
