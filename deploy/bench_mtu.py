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
Entrouter Line - MTU Discovery + Throughput Benchmark
Tests various payload sizes to find what works through the tunnel,
then measures throughput at the optimal size.
"""
import socket
import time
import sys
import threading

HOST = '127.0.0.1'
PORT = 8443
LABEL = sys.argv[1] if len(sys.argv) > 1 else 'NODE'
WAIT_FOR_PAIR = float(sys.argv[2]) if len(sys.argv) > 2 else 3.0

def test_payload_size(size, timeout=5.0):
    """Test if a payload of given size makes it through the relay."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.settimeout(timeout)
    try:
        sock.connect((HOST, PORT))
        # Wait for paired connection on other side
        time.sleep(WAIT_FOR_PAIR)
        
        payload = bytes([i % 256 for i in range(size)])
        sock.sendall(payload)
        
        try:
            data = sock.recv(65536)
            if data:
                return len(data)
        except socket.timeout:
            pass
        return 0
    except Exception as e:
        print(f'  Error: {e}')
        return -1
    finally:
        sock.close()

def throughput_test(chunk_size, duration=10):
    """Measure throughput at a given chunk size."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.settimeout(duration + 10)
    sock.connect((HOST, PORT))
    
    print(f'[{LABEL}] Connected for throughput test (chunk={chunk_size}B, {duration}s)')
    time.sleep(WAIT_FOR_PAIR)
    
    tx_results = {}
    rx_results = {}
    
    def sender():
        data = b'T' * chunk_size
        start = time.time()
        total = 0
        sends = 0
        while time.time() - start < duration:
            try:
                sock.sendall(data)
                total += chunk_size
                sends += 1
            except:
                break
        tx_results['bytes'] = total
        tx_results['sends'] = sends
        tx_results['elapsed'] = time.time() - start
    
    def receiver():
        start = time.time()
        total = 0
        recvs = 0
        while time.time() - start < duration + 3:
            try:
                data = sock.recv(65536)
                if not data:
                    break
                total += len(data)
                recvs += 1
            except:
                break
        rx_results['bytes'] = total
        rx_results['recvs'] = recvs
        rx_results['elapsed'] = time.time() - start
    
    tr = threading.Thread(target=receiver)
    ts = threading.Thread(target=sender)
    tr.start()
    time.sleep(0.1)
    ts.start()
    ts.join()
    tr.join(timeout=duration + 15)
    sock.close()
    
    return tx_results, rx_results

# === Phase 1: MTU Discovery ===
print(f'=== [{LABEL}] MTU DISCOVERY ===')
print(f'(Each test connects, waits {WAIT_FOR_PAIR}s for pair, sends payload, checks receipt)')
print()

sizes = [16, 64, 256, 512, 1024, 1200, 1400, 2048, 4096, 8192, 16384]
working_sizes = []

for size in sizes:
    rx = test_payload_size(size, timeout=8.0)
    status = 'OK' if rx > 0 else ('FAIL' if rx == 0 else 'ERR')
    print(f'  {size:>6}B payload -> {status} (received {rx}B)')
    if rx > 0:
        working_sizes.append(size)

print()
if working_sizes:
    best = max(working_sizes)
    print(f'Max working payload: {best}B')
    
    # === Phase 2: Throughput at best size ===
    print(f'\n=== [{LABEL}] THROUGHPUT TEST ({best}B chunks, 10s) ===')
    tx, rx = throughput_test(best, 10)
    
    tx_bytes = tx.get('bytes', 0)
    tx_elapsed = tx.get('elapsed', 1)
    rx_bytes = rx.get('bytes', 0)
    rx_elapsed = rx.get('elapsed', 1)
    
    print(f'TX: {tx_bytes:,} bytes in {tx_elapsed:.2f}s = {tx_bytes/(tx_elapsed*1e6):.2f} MB/s')
    print(f'TX: {tx.get("sends",0):,} sends = {tx.get("sends",0)/tx_elapsed:.0f}/sec')
    if rx_bytes > 0:
        print(f'RX: {rx_bytes:,} bytes in {rx_elapsed:.2f}s = {rx_bytes/(rx_elapsed*1e6):.2f} MB/s')
        print(f'RX: {rx.get("recvs",0):,} recvs = {rx.get("recvs",0)/rx_elapsed:.0f}/sec')
    else:
        print(f'RX: No data received')
else:
    print('NO payload sizes worked! Check relay configuration.')

print(f'\n=== [{LABEL}] DONE ===')
