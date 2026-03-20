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
Entrouter Line - Throughput Benchmark
Measures relay throughput by sending bulk data through the TCP edge.
Run simultaneously on both nodes for bidirectional measurement,
or on one node for unidirectional measurement.
"""
import socket
import time
import sys
import threading

# Configuration
HOST = '127.0.0.1'
PORT = 8443
CHUNK_SIZE = 8192        # 8KB per send
DURATION = 10            # seconds
LABEL = sys.argv[1] if len(sys.argv) > 1 else 'NODE'

def sender(sock, results):
    """Send as much data as possible for DURATION seconds."""
    data = b'X' * CHUNK_SIZE
    start = time.time()
    total_bytes = 0
    total_sends = 0

    while time.time() - start < DURATION:
        try:
            sock.sendall(data)
            total_bytes += CHUNK_SIZE
            total_sends += 1
        except Exception as e:
            print(f'[{LABEL}] Send error after {total_sends} sends: {e}')
            break

    elapsed = time.time() - start
    results['tx_bytes'] = total_bytes
    results['tx_sends'] = total_sends
    results['tx_elapsed'] = elapsed

def receiver(sock, results):
    """Receive data for DURATION seconds."""
    start = time.time()
    total_bytes = 0
    total_recvs = 0
    first_byte_time = None

    sock.settimeout(DURATION + 5)
    while time.time() - start < DURATION + 2:
        try:
            data = sock.recv(65536)
            if not data:
                break
            if first_byte_time is None:
                first_byte_time = time.time()
            total_bytes += len(data)
            total_recvs += 1
        except socket.timeout:
            break
        except Exception as e:
            print(f'[{LABEL}] Recv error: {e}')
            break

    elapsed = time.time() - start
    results['rx_bytes'] = total_bytes
    results['rx_recvs'] = total_recvs
    results['rx_elapsed'] = elapsed
    if first_byte_time:
        results['first_byte_latency'] = first_byte_time - start

def run_benchmark():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.connect((HOST, PORT))
    print(f'[{LABEL}] Connected, starting {DURATION}s benchmark...')

    tx_results = {}
    rx_results = {}

    # Start sender and receiver threads
    t_send = threading.Thread(target=sender, args=(sock, tx_results))
    t_recv = threading.Thread(target=receiver, args=(sock, rx_results))

    t_recv.start()
    time.sleep(0.5)  # let receiver start first
    t_send.start()

    t_send.join()
    t_recv.join(timeout=DURATION + 10)

    sock.close()

    # Report
    print(f'\n=== [{LABEL}] THROUGHPUT RESULTS ===')

    tx_bytes = tx_results.get('tx_bytes', 0)
    tx_elapsed = tx_results.get('tx_elapsed', 1)
    tx_sends = tx_results.get('tx_sends', 0)
    tx_mbps = (tx_bytes * 8) / (tx_elapsed * 1_000_000)
    tx_mbs = tx_bytes / (tx_elapsed * 1_000_000)

    print(f'TX: {tx_bytes:,} bytes in {tx_elapsed:.2f}s')
    print(f'TX: {tx_mbs:.2f} MB/s ({tx_mbps:.2f} Mbps)')
    print(f'TX: {tx_sends:,} sends ({tx_sends/tx_elapsed:.0f} sends/sec)')

    rx_bytes = rx_results.get('rx_bytes', 0)
    rx_elapsed = rx_results.get('rx_elapsed', 1)
    rx_recvs = rx_results.get('rx_recvs', 0)
    if rx_bytes > 0:
        rx_mbps = (rx_bytes * 8) / (rx_elapsed * 1_000_000)
        rx_mbs = rx_bytes / (rx_elapsed * 1_000_000)
        print(f'RX: {rx_bytes:,} bytes in {rx_elapsed:.2f}s')
        print(f'RX: {rx_mbs:.2f} MB/s ({rx_mbps:.2f} Mbps)')
        print(f'RX: {rx_recvs:,} recvs ({rx_recvs/rx_elapsed:.0f} recvs/sec)')
    else:
        print(f'RX: No data received (expected if no paired sender)')

    if 'first_byte_latency' in rx_results:
        print(f'First byte latency: {rx_results["first_byte_latency"]*1000:.1f}ms')

    print(f'=== END ===')

if __name__ == '__main__':
    run_benchmark()
