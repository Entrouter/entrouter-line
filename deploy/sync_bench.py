#!/usr/bin/env python3
# Copyright 2026 John A Keeney - Entrouter
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""
Synchronized relay benchmark.
Both sides connect, exchange READY signals, then send data simultaneously.

Usage:
  python3 sync_bench.py --role sender   --rate-mbps 50 --duration 10
  python3 sync_bench.py --role receiver  --rate-mbps 50 --duration 10

The sender connects first, the receiver 0.5s later.
Both sides:
  1. Connect to localhost:8443
  2. Send b"READY\\n" (6 bytes)
  3. Wait to receive b"READY\\n" from the other side (proves relay is live)
  4. Then run the benchmark: send chunks at --rate-mbps for --duration seconds

Default: --rate-mbps 0 = full blast, --chunk-size 1024
"""
import argparse, socket, time, struct, sys, threading

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--role", required=True, choices=["sender", "receiver"])
    p.add_argument("--rate-mbps", type=float, default=0, help="0 = full blast")
    p.add_argument("--duration", type=float, default=10)
    p.add_argument("--chunk-size", type=int, default=1024)
    p.add_argument("--port", type=int, default=8443)
    args = p.parse_args()

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.connect(("127.0.0.1", args.port))
    sock.settimeout(30)
    print(f"[{args.role}] connected to 127.0.0.1:{args.port}")

    # Wait for both sides' TCP connections to exist in the relay
    # before sending any data. This ensures flow_id=1 exists on both nodes.
    settle = 3
    print(f"[{args.role}] waiting {settle}s for both sides to connect...")
    time.sleep(settle)

    # --- Handshake: exchange READY signals through the relay ---
    # Retry READY to survive packet loss (UDP tunnel is unreliable)
    sock.settimeout(2)
    buf = b""
    for attempt in range(15):
        sock.sendall(b"READY\n")
        if attempt == 0:
            print(f"[{args.role}] sent READY, waiting for peer...")
        try:
            while True:
                chunk = sock.recv(4096)
                if not chunk:
                    print(f"[{args.role}] connection closed during handshake")
                    return
                buf += chunk
                if b"READY\n" in buf:
                    break
        except socket.timeout:
            pass
        if b"READY\n" in buf:
            break
    else:
        print(f"[{args.role}] TIMEOUT waiting for peer READY after 15 attempts")
        return
    sock.settimeout(30)

    print(f"[{args.role}] peer READY received - starting benchmark")
    # Any leftover data after READY\n is the start of benchmark data
    leftover = buf.split(b"READY\n", 1)[1]

    # --- Benchmark phase ---
    total_sent = 0
    total_recv = 0
    recv_count = 0
    chunks_sent = 0
    payload = bytes(range(256)) * (args.chunk_size // 256 + 1)
    payload = payload[:args.chunk_size]

    # Calculate inter-chunk delay for rate limiting
    if args.rate_mbps > 0:
        bytes_per_sec = args.rate_mbps * 1_000_000 / 8
        delay = args.chunk_size / bytes_per_sec
    else:
        delay = 0

    done = threading.Event()
    errors = []

    def recv_thread():
        nonlocal total_recv, recv_count, leftover
        sock_r = sock  # same socket
        # Process any leftover from handshake
        if leftover:
            total_recv += len(leftover)
            recv_count += 1

        while not done.is_set():
            try:
                sock_r.settimeout(1.0)
                data = sock_r.recv(65536)
                if not data:
                    break
                total_recv += len(data)
                recv_count += 1
            except socket.timeout:
                continue
            except Exception as e:
                errors.append(str(e))
                break

    rt = threading.Thread(target=recv_thread, daemon=True)
    rt.start()

    # Send phase
    start = time.monotonic()
    try:
        while time.monotonic() - start < args.duration:
            sock.sendall(payload)
            total_sent += len(payload)
            chunks_sent += 1
            if delay > 0:
                time.sleep(delay)
    except Exception as e:
        errors.append(f"send error: {e}")

    elapsed_send = time.monotonic() - start
    print(f"[{args.role}] send done: {total_sent:,} bytes ({chunks_sent} chunks) in {elapsed_send:.1f}s")

    # Wait for drain
    time.sleep(2)
    done.set()
    rt.join(timeout=3)

    total_elapsed = time.monotonic() - start
    send_mbps = (total_sent * 8 / elapsed_send / 1_000_000) if elapsed_send > 0 else 0
    recv_mbps = (total_recv * 8 / total_elapsed / 1_000_000) if total_elapsed > 0 else 0
    loss_pct = ((total_sent - total_recv) / total_sent * 100) if total_sent > 0 else 0

    print(f"[{args.role}] RESULTS:")
    print(f"  TX: {total_sent:>12,} bytes  ({send_mbps:.1f} Mbps)")
    print(f"  RX: {total_recv:>12,} bytes  ({recv_mbps:.1f} Mbps, {recv_count} recvs)")
    if errors:
        print(f"  ERRORS: {errors}")

    # Print machine-readable summary
    print(f"SUMMARY|{args.role}|tx={total_sent}|rx={total_recv}|send_mbps={send_mbps:.1f}|recv_mbps={recv_mbps:.1f}|chunks={chunks_sent}|duration={total_elapsed:.1f}")

    sock.close()

if __name__ == "__main__":
    main()
