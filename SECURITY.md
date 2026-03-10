# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅ Current |

## Reporting a Vulnerability

If you discover a security vulnerability in Entrouter Line, **please do not open a public issue.**

Instead, report it privately:

1. **Email:** Send details to the maintainers via [GitHub private vulnerability reporting](https://github.com/Entrouter/entrouter-line/security/advisories/new)
2. Include: affected component, reproduction steps, and potential impact
3. We will acknowledge receipt within 48 hours
4. We aim to release a fix within 7 days of confirmation

## Scope

The following are in scope:

- Encryption bypass or key leakage in the relay tunnel (`src/relay/crypto.rs`)
- Wire protocol parsing vulnerabilities (`src/relay/wire.rs`)
- FEC implementation flaws that could cause data corruption (`src/relay/fec.rs`)
- Admin API exposure beyond localhost (`src/admin.rs`)
- Any path that allows unauthenticated packet injection into the mesh

## Design Principles

- **Encryption is always on.** There is no plaintext mode. All inter-PoP traffic uses ChaCha20-Poly1305 with pre-shared keys.
- **Admin API binds to localhost only.** It is not exposed to the network by default.
- **No dynamic code execution.** The relay processes fixed-format binary packets only.
