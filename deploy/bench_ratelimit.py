# Copyright 2025 John A Keeney - Entrouter
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
Entrouter Line - Rate-Limited Throughput Benchmark
Sends at a controlled rate to find sustainable zero-loss throughput.
Run simultaneously on both nodes.
"""
import socket
import time
import sys
import threading
import struct

HOST = '127.0.0.1'
PORT = 8443
CHUNK_SIZE = 1024          # 1KB per send (small to stay in single relay packet)
DURATION = 10              # seconds
LABEL = sys.argv[1] if len(sys.argv) > 1 else 'NODE'
TARGET_MBPS = float(sys.argv[2]) if len(sys.argv) > 2 else 50.0

def sender(sock, results):
    """Send at a controlled rate for DURATION seconds."""
    # Prepend a 4-byte sequence number so receiver can detect loss
    seq = 0
    target_bytes_per_sec = TARGET_MBPS * 1_000_000 / 8
    sends_per_sec = target_bytes_per_sec / CHUNK_SIZE
    interval = 1.0 / sends_per_sec if sends_per_sec > 0 else 0.001

    start = time.time()
    total_bytes = 0
    total_sends = 0
    next_send = start

    while time.time() - start < DURATION:
        now = time.time()
        if now < next_send:
            # Busy-wait for precise timing (sleep is too coarse)
            continue

        data = struct.pack('<I', seq) + b'X' * (CHUNK_SIZE - 4)
        try:
            sock.sendall(data)
            total_bytes += CHUNK_SIZE
            total_sends += 1
            seq += 1
        except Exception as e:
            print(f'[{LABEL}] Send error after {total_sends} sends: {e}')
            break
        next_send += interval

    elapsed = time.time() - start
    results['tx_bytes'] = total_bytes
    results['tx_sends'] = total_sends
    results['tx_elapsed'] = elapsed
    results['tx_seq'] = seq

def receiver(sock, results):
    """Receive data for DURATION seconds, tracking sequence for loss detection."""
    start = time.time()
    total_bytes = 0
    total_recvs = 0
    first_byte_time = None
    max_seq = -1
    seen_seqs = set()

    sock.settimeout(DURATION + 5)
    buf = b''
    while time.time() - start < DURATION + 3:
        try:
            data = sock.recv(65536)
            if not data:
                break
            if first_byte_time is None:
                first_byte_time = time.time()
            total_bytes += len(data)
            total_recvs += 1

            # Parse sequence numbers from received chunks
            buf += data
            while len(buf) >= CHUNK_SIZE:
                chunk = buf[:CHUNK_SIZE]
                buf = buf[CHUNK_SIZE:]
                seq = struct.unpack('<I', chunk[:4])[0]
                seen_seqs.add(seq)
                if seq > max_seq:
                    max_seq = seq
        except socket.timeout:
            break
        except Exception as e:
            print(f'[{LABEL}] Recv error: {e}')
            break

    elapsed = time.time() - start
    results['rx_bytes'] = total_bytes
    results['rx_recvs'] = total_recvs
    results['rx_elapsed'] = elapsed
    results['rx_max_seq'] = max_seq
    results['rx_unique_seqs'] = len(seen_seqs)
    if first_byte_time:
        results['first_byte_latency'] = first_byte_time - start
    # Calculate loss: if max_seq is N, we expect seqs 0..N
    if max_seq >= 0:
        expected = max_seq + 1
        results['rx_loss_pct'] = (1.0 - len(seen_seqs) / expected) * 100 if expected > 0 else 0

def run_benchmark():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.connect((HOST, PORT))
    print(f'[{LABEL}] Connected, target {TARGET_MBPS} Mbps, {DURATION}s...')

    tx_results = {}
    rx_results = {}

    t_recv = threading.Thread(target=receiver, args=(sock, rx_results))
    t_recv.start()
    time.sleep(0.5)
    t_send = threading.Thread(target=sender, args=(sock, tx_results))
    t_send.start()

    t_send.join()
    t_recv.join(timeout=DURATION + 10)
    sock.close()

    # Report
    print(f'\n=== [{LABEL}] RATE-LIMITED RESULTS ({TARGET_MBPS} Mbps target) ===')

    tx_bytes = tx_results.get('tx_bytes', 0)
    tx_elapsed = tx_results.get('tx_elapsed', 1)
    tx_sends = tx_results.get('tx_sends', 0)
    tx_mbps = (tx_bytes * 8) / (tx_elapsed * 1_000_000) if tx_elapsed > 0 else 0
    print(f'TX: {tx_bytes:,} bytes in {tx_elapsed:.2f}s = {tx_mbps:.1f} Mbps ({tx_sends:,} sends)')

    rx_bytes = rx_results.get('rx_bytes', 0)
    rx_elapsed = rx_results.get('rx_elapsed', 1)
    rx_unique = rx_results.get('rx_unique_seqs', 0)
    rx_max = rx_results.get('rx_max_seq', -1)
    loss_pct = rx_results.get('rx_loss_pct', 0)

    if rx_bytes > 0:
        rx_mbps = (rx_bytes * 8) / (rx_elapsed * 1_000_000) if rx_elapsed > 0 else 0
        print(f'RX: {rx_bytes:,} bytes in {rx_elapsed:.2f}s = {rx_mbps:.1f} Mbps')
        print(f'RX: {rx_unique}/{rx_max+1 if rx_max >= 0 else 0} chunks received')
        print(f'Loss: {loss_pct:.2f}%')
    else:
        print(f'RX: No data received')

    if 'first_byte_latency' in rx_results:
        print(f'First byte latency: {rx_results["first_byte_latency"]*1000:.1f}ms')
    print(f'=== END ===')

if __name__ == '__main__':
    run_benchmark()
