# --- Build ---
FROM rust:1.87-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY benches/ benches/
RUN cargo build --release

# --- Runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/entrouter-line /usr/local/bin/entrouter-line

# relay=4433/udp  tcp-edge=8443  quic-edge=4434/udp  admin=9090
EXPOSE 4433/udp 8443/tcp 4434/udp 9090/tcp

ENTRYPOINT ["entrouter-line"]
CMD ["--config", "/etc/entrouter/config.toml"]
