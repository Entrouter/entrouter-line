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
A/B Benchmark: Entrouter Relay vs Direct TCP

Compares relay-tunnelled latency & throughput against raw TCP across an
intercontinental link, with simulated packet loss via tc netem.

Set the following environment variables before running:

    export RELAY_NODE_A_HOST=<ip>
    export RELAY_NODE_A_USER=root
    export RELAY_NODE_A_PASSWORD=<password>
    export RELAY_NODE_B_HOST=<ip>
    export RELAY_NODE_B_USER=root
    export RELAY_NODE_B_PASSWORD=<password>

Requires: paramiko (pip install paramiko)
"""
import paramiko, time, json, sys, os


def _env(name):
    val = os.environ.get(name)
    if not val:
        print(f"ERROR: environment variable {name} is not set", file=sys.stderr)
        sys.exit(1)
    return val


NODE_A = {
    "host": _env("RELAY_NODE_A_HOST"),
    "user": os.environ.get("RELAY_NODE_A_USER", "root"),
    "password": _env("RELAY_NODE_A_PASSWORD"),
}
NODE_B = {
    "host": _env("RELAY_NODE_B_HOST"),
    "user": os.environ.get("RELAY_NODE_B_USER", "root"),
    "password": _env("RELAY_NODE_B_PASSWORD"),
}

def ssh(srv):
    c = paramiko.SSHClient()
    c.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    c.connect(srv["host"], username=srv["user"], password=srv["password"], timeout=30)
    return c

def run(c, cmd, timeout=30):
    _, o, e = c.exec_command(cmd, timeout=timeout)
    return o.read().decode(errors="replace").strip(), e.read().decode(errors="replace").strip()

def restart_relays(node_a, node_b):
    """Kill and restart relays on both sides."""
    for c in [node_a, node_b]:
        run(c, "pkill -9 -f 'entrouter-line --config' 2>/dev/null; true")
    time.sleep(2)
    for c in [node_a, node_b]:
        c.exec_command(
            "cd /opt/entrouter-line && RUST_LOG=info "
            "nohup ./target/release/entrouter-line --config config.toml "
            "> /tmp/entrouter.log 2>&1 &"
        )
    time.sleep(4)
    # Verify health
    for name, c in [("NODE_A", node_a), ("NODE_B", node_b)]:
        h, _ = run(c, "curl -s http://127.0.0.1:9090/health")
        if h != "ok":
            raise RuntimeError(f"{name} relay not healthy: {h}")
    # Wait for mesh convergence
    time.sleep(6)
    s, _ = run(node_a, "curl -s http://127.0.0.1:9090/status")
    status = json.loads(s)
    if status["peers"] < 1:
        raise RuntimeError(f"Mesh not converged: {s}")
    rtt = status["latencies"][0]["rtt_us"]
    return rtt

# ===== BENCHMARK SCRIPTS =====

# Echo relay client: sits on Node B:8443, echoes everything back through relay
ECHO_RELAY_CLIENT = r'''
import socket, select
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
sock.settimeout(120)
sock.connect(("127.0.0.1", 8443))
print("ECHO_CONNECTED", flush=True)
try:
    while True:
        data = sock.recv(65536)
        if not data:
            break
        sock.sendall(data)
except Exception as e:
    print(f"ECHO_ERROR: {e}", flush=True)
finally:
    sock.close()
'''

# Latency benchmark: sends small messages, measures RTT
LATENCY_BENCH = r'''
import socket, time, sys, struct, json

TARGET = sys.argv[1]   # host:port
ROUNDS = int(sys.argv[2])
WARMUP = int(sys.argv[3])
PAYLOAD = b"P" * 64    # 64 byte ping

host, port = TARGET.split(":")
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
sock.settimeout(30)
sock.connect((host, int(port)))

# Length-prefixed protocol: [4 bytes len][payload]
def send_msg(s, data):
    s.sendall(struct.pack("!I", len(data)) + data)

def recv_msg(s):
    hdr = b""
    while len(hdr) < 4:
        chunk = s.recv(4 - len(hdr))
        if not chunk:
            raise ConnectionError("closed")
        hdr += chunk
    ln = struct.unpack("!I", hdr)[0]
    buf = b""
    while len(buf) < ln:
        chunk = s.recv(min(ln - len(buf), 65536))
        if not chunk:
            raise ConnectionError("closed")
        buf += chunk
    return buf

# Warmup
for _ in range(WARMUP):
    send_msg(sock, PAYLOAD)
    recv_msg(sock)

# Measure
rtts = []
for i in range(ROUNDS):
    t0 = time.perf_counter()
    send_msg(sock, PAYLOAD)
    recv_msg(sock)
    rtt_ms = (time.perf_counter() - t0) * 1000
    rtts.append(rtt_ms)

sock.close()

rtts.sort()
result = {
    "rounds": len(rtts),
    "min_ms": round(rtts[0], 2),
    "p50_ms": round(rtts[len(rtts)//2], 2),
    "p95_ms": round(rtts[int(len(rtts)*0.95)], 2),
    "p99_ms": round(rtts[int(len(rtts)*0.99)], 2),
    "max_ms": round(rtts[-1], 2),
    "mean_ms": round(sum(rtts)/len(rtts), 2),
}
print(json.dumps(result))
'''

# Throughput benchmark: sequential send/recv (no pipelining) with length-prefix
THROUGHPUT_BENCH = r'''
import socket, time, sys, struct, json

TARGET = sys.argv[1]       # host:port
ROUNDS = int(sys.argv[2])  # number of round-trips
MSG_SIZE = int(sys.argv[3])  # bytes per message

host, port = TARGET.split(":")
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
sock.settimeout(30)
sock.connect((host, int(port)))

payload = b"T" * MSG_SIZE

def send_msg(s, data):
    s.sendall(struct.pack("!I", len(data)) + data)

def recv_msg(s):
    hdr = b""
    while len(hdr) < 4:
        chunk = s.recv(4 - len(hdr))
        if not chunk: raise ConnectionError("closed")
        hdr += chunk
    ln = struct.unpack("!I", hdr)[0]
    buf = b""
    while len(buf) < ln:
        chunk = s.recv(min(ln - len(buf), 65536))
        if not chunk: raise ConnectionError("closed")
        buf += chunk
    return buf

ok = 0
errors = []
t0 = time.perf_counter()
for i in range(ROUNDS):
    try:
        send_msg(sock, payload)
        resp = recv_msg(sock)
        if len(resp) == MSG_SIZE:
            ok += 1
    except Exception as e:
        errors.append(str(e))
        break
elapsed = time.perf_counter() - t0
sock.close()

total_bytes = ok * MSG_SIZE
result = {
    "rounds": ROUNDS,
    "msg_size": MSG_SIZE,
    "completed": ok,
    "total_bytes": total_bytes,
    "elapsed_s": round(elapsed, 3),
    "goodput_mbps": round(total_bytes * 8 / elapsed / 1_000_000, 2) if elapsed > 0 else 0,
    "msgs_per_sec": round(ok / elapsed, 1) if elapsed > 0 else 0,
    "delivery_pct": round(ok / ROUNDS * 100, 1),
    "errors": errors,
}
print(json.dumps(result))
'''

# Echo server on Node B - length-prefixed protocol matching latency bench
ECHO_SERVER_LP = r'''
import socket, struct, threading, sys

def handle(conn, addr):
    try:
        while True:
            hdr = b""
            while len(hdr) < 4:
                chunk = conn.recv(4 - len(hdr))
                if not chunk:
                    return
                hdr += chunk
            ln = struct.unpack("!I", hdr)[0]
            buf = b""
            while len(buf) < ln:
                chunk = conn.recv(min(ln - len(buf), 65536))
                if not chunk:
                    return
                buf += chunk
            conn.sendall(struct.pack("!I", ln) + buf)
    except Exception:
        pass
    finally:
        conn.close()

srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("0.0.0.0", 7777))
srv.listen(16)
print("ECHO_LP_LISTENING", flush=True)
while True:
    conn, addr = srv.accept()
    threading.Thread(target=handle, args=(conn, addr), daemon=True).start()
'''

# Raw echo server for throughput test (no framing)
ECHO_SERVER_RAW = r'''
import socket, threading, sys

def handle(conn, addr):
    try:
        while True:
            data = conn.recv(65536)
            if not data:
                break
            conn.sendall(data)
    except Exception:
        pass
    finally:
        conn.close()

srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("0.0.0.0", 7778))
srv.listen(16)
print("ECHO_RAW_LISTENING", flush=True)
while True:
    conn, addr = srv.accept()
    threading.Thread(target=handle, args=(conn, addr), daemon=True).start()
'''

# Echo relay client with length-prefix protocol (matches latency bench)
ECHO_RELAY_LP = r'''
import socket, struct
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
sock.settimeout(300)
sock.connect(("127.0.0.1", 8443))
print("ECHO_RELAY_LP_CONNECTED", flush=True)
try:
    while True:
        hdr = b""
        while len(hdr) < 4:
            chunk = sock.recv(4 - len(hdr))
            if not chunk:
                raise ConnectionError("closed")
            hdr += chunk
        ln = struct.unpack("!I", hdr)[0]
        buf = b""
        while len(buf) < ln:
            chunk = sock.recv(min(ln - len(buf), 65536))
            if not chunk:
                raise ConnectionError("closed")
            buf += chunk
        sock.sendall(struct.pack("!I", ln) + buf)
except Exception as e:
    print(f"ECHO_RELAY_LP_ERROR: {e}", flush=True)
finally:
    sock.close()
'''

# Echo relay client raw (for throughput test)
ECHO_RELAY_RAW = r'''
import socket
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 262144)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 262144)
sock.settimeout(300)
sock.connect(("127.0.0.1", 8443))
print("ECHO_RELAY_RAW_CONNECTED", flush=True)
try:
    while True:
        data = sock.recv(65536)
        if not data:
            break
        sock.sendall(data)
except Exception as e:
    print(f"ECHO_RELAY_RAW_ERROR: {e}", flush=True)
finally:
    sock.close()
'''

def upload_script(c, path, content):
    sftp = c.open_sftp()
    with sftp.file(path, "w") as f:
        f.write(content)
    sftp.close()

def wait_for_line(c, log_path, marker, timeout=30):
    """Wait for a marker line to appear in a log file."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        o, _ = run(c, f"cat {log_path}")
        if marker in o:
            return True
        time.sleep(1)
    return False

def get_iface(c):
    """Get the main network interface name."""
    cmd = 'ip route get 8.8.8.8 | grep -oP "dev \\K\\S+"'
    o, _ = run(c, cmd)
    return o.strip() or "enp1s0"

def apply_netem(c, loss_pct):
    """Apply packet loss with tc netem. Returns True if successful."""
    iface = get_iface(c)
    run(c, f"tc qdisc del dev {iface} root 2>/dev/null; true")
    if loss_pct > 0:
        o, e = run(c, f"tc qdisc add dev {iface} root netem loss {loss_pct}%")
        if "Cannot find" in o or "RTNETLINK" in e:
            return False
        # Verify
        o, _ = run(c, f"tc qdisc show dev {iface}")
        return "netem" in o
    return True

def clear_netem(c):
    iface = get_iface(c)
    run(c, f"tc qdisc del dev {iface} root 2>/dev/null; true")

# ===== MAIN =====

print("=" * 60)
print("  ENTROUTER RELAY vs DIRECT TCP  A/B BENCHMARK")
print("  Node A <-> Node B")
print("=" * 60)

node_a = ssh(NODE_A)
node_b = ssh(NODE_B)

# Kill any old processes
run(node_b, "pkill -f 'echo_server\\|echo_relay\\|echo_lp\\|echo_raw' 2>/dev/null; true")
run(node_a, "pkill -f 'latency_bench\\|throughput_bench' 2>/dev/null; true")
clear_netem(node_b)
clear_netem(node_a)

# Upload scripts
print("\n[*] Uploading scripts...")
upload_script(node_b, "/tmp/echo_server_lp.py", ECHO_SERVER_LP)
upload_script(node_b, "/tmp/echo_server_raw.py", ECHO_SERVER_RAW)
upload_script(node_b, "/tmp/echo_relay_lp.py", ECHO_RELAY_LP)
upload_script(node_b, "/tmp/echo_relay_raw.py", ECHO_RELAY_RAW)
upload_script(node_a, "/tmp/latency_bench.py", LATENCY_BENCH)
upload_script(node_a, "/tmp/throughput_bench.py", THROUGHPUT_BENCH)

# Start echo servers on Node B
print("[*] Starting echo servers on Node B...")
run(node_b, "pkill -f echo_server_lp.py 2>/dev/null; true")
run(node_b, "pkill -f echo_server_raw.py 2>/dev/null; true")
time.sleep(1)
node_b.exec_command("nohup python3 /tmp/echo_server_lp.py > /tmp/echo_server_lp.log 2>&1 &")
node_b.exec_command("nohup python3 /tmp/echo_server_raw.py > /tmp/echo_server_raw.log 2>&1 &")
time.sleep(2)
o, _ = run(node_b, "ss -tlnp | grep -E '7777|7778'")
print(f"  Echo servers: {o}")

# Check if tc/netem is available
can_netem = apply_netem(node_b, 1)
clear_netem(node_b)
if can_netem:
    loss_levels = [0, 1, 3, 5]
    print("[*] tc netem available - will test loss levels: 0%, 1%, 3%, 5%")
else:
    loss_levels = [0]
    print("[!] tc netem NOT available - baseline only (no simulated loss)")

results = []

for loss_pct in loss_levels:
    print(f"\n{'='*60}")
    print(f"  TEST: {loss_pct}% packet loss")
    print(f"{'='*60}")

    # Restart relays fresh
    print("  [*] Restarting relays...")
    rtt_us = restart_relays(node_a, node_b)
    print(f"  [*] Mesh RTT: {rtt_us}µs")

    # Apply netem on Node B's outbound (affects both directions of relay)
    if loss_pct > 0:
        apply_netem(node_b, loss_pct)
        print(f"  [*] Applied {loss_pct}% packet loss on Node B")

    # --- LATENCY: RELAY ---
    print("  [*] Latency test: RELAY")
    # Start echo relay client (length-prefixed) on Node B
    run(node_b, "pkill -f echo_relay_lp.py 2>/dev/null; true")
    time.sleep(1)
    node_b.exec_command("nohup python3 /tmp/echo_relay_lp.py > /tmp/echo_relay_lp.log 2>&1 &")
    time.sleep(3)
    o, _ = run(node_b, "cat /tmp/echo_relay_lp.log")
    if "CONNECTED" not in o:
        print(f"  [!] Echo relay LP client failed: {o}")
        run(node_b, "pkill -f echo_relay_lp.py 2>/dev/null; true")
        clear_netem(node_b)
        results.append({"loss_pct": loss_pct, "error": "echo relay LP failed"})
        continue

    # Run latency bench from Node A
    relay_lat_out, relay_lat_err = run(node_a, "python3 /tmp/latency_bench.py 127.0.0.1:8443 20 2", timeout=60)
    try:
        relay_lat = json.loads(relay_lat_out)
        print(f"    Relay latency: p50={relay_lat['p50_ms']}ms  p95={relay_lat['p95_ms']}ms  p99={relay_lat['p99_ms']}ms  mean={relay_lat['mean_ms']}ms")
    except Exception as e:
        print(f"    Relay latency FAILED: {relay_lat_out[:200]} | {relay_lat_err[:200]}")
        relay_lat = {"error": str(e)}

    # Kill echo relay LP
    run(node_b, "pkill -f echo_relay_lp.py 2>/dev/null; true")
    time.sleep(1)

    # --- LATENCY: DIRECT ---
    print("  [*] Latency test: DIRECT TCP")
    direct_lat_out, direct_lat_err = run(node_a, f"python3 /tmp/latency_bench.py {NODE_B['host']}:7777 20 2", timeout=60)
    try:
        direct_lat = json.loads(direct_lat_out)
        print(f"    Direct latency: p50={direct_lat['p50_ms']}ms  p95={direct_lat['p95_ms']}ms  p99={direct_lat['p99_ms']}ms  mean={direct_lat['mean_ms']}ms")
    except Exception as e:
        print(f"    Direct latency FAILED: {direct_lat_out[:200]} | {direct_lat_err[:200]}")
        direct_lat = {"error": str(e)}

    # --- THROUGHPUT: RELAY ---
    print("  [*] Throughput test: RELAY (20 x 512B sequential)")
    # Need fresh relay connections - restart relays again for clean flow IDs
    print("    Restarting relays for throughput...")
    restart_relays(node_a, node_b)
    if loss_pct > 0:
        apply_netem(node_b, loss_pct)

    run(node_b, "pkill -f echo_relay_lp.py 2>/dev/null; true")
    time.sleep(1)
    node_b.exec_command("nohup python3 /tmp/echo_relay_lp.py > /tmp/echo_relay_lp2.log 2>&1 &")
    time.sleep(3)
    o, _ = run(node_b, "cat /tmp/echo_relay_lp2.log")
    if "CONNECTED" not in o:
        print(f"    [!] Echo relay LP client failed: {o}")
        relay_tp = {"error": "echo relay LP failed"}
    else:
        relay_tp_out, relay_tp_err = run(node_a, "python3 /tmp/throughput_bench.py 127.0.0.1:8443 20 512", timeout=60)
        try:
            relay_tp = json.loads(relay_tp_out)
            print(f"    Relay: {relay_tp['completed']}/{relay_tp['rounds']} msgs  goodput={relay_tp['goodput_mbps']}Mbps  {relay_tp['msgs_per_sec']}msg/s  {relay_tp['elapsed_s']}s")
        except Exception as e:
            print(f"    Relay throughput FAILED: {relay_tp_out[:200]} | {relay_tp_err[:200]}")
            relay_tp = {"error": str(e)}

    run(node_b, "pkill -f echo_relay_lp.py 2>/dev/null; true")
    time.sleep(1)

    # --- THROUGHPUT: DIRECT ---
    print("  [*] Throughput test: DIRECT TCP (20 x 512B sequential)")
    direct_tp_out, direct_tp_err = run(node_a, f"python3 /tmp/throughput_bench.py {NODE_B['host']}:7777 20 512", timeout=60)
    try:
        direct_tp = json.loads(direct_tp_out)
        print(f"    Direct: {direct_tp['completed']}/{direct_tp['rounds']} msgs  goodput={direct_tp['goodput_mbps']}Mbps  {direct_tp['msgs_per_sec']}msg/s  {direct_tp['elapsed_s']}s")
    except Exception as e:
        print(f"    Direct throughput FAILED: {direct_tp_out[:200]} | {direct_tp_err[:200]}")
        direct_tp = {"error": str(e)}

    # Clean up netem
    clear_netem(node_b)

    results.append({
        "loss_pct": loss_pct,
        "mesh_rtt_us": rtt_us,
        "relay_latency": relay_lat,
        "direct_latency": direct_lat,
        "relay_throughput": relay_tp,
        "direct_throughput": direct_tp,
    })

# ===== RESULTS SUMMARY =====
print("\n" + "=" * 80)
print("  RESULTS SUMMARY: Entrouter Relay vs Direct TCP")
print("=" * 80)

# Latency table
print(f"\n{'LATENCY (ms)':<20} {'':>8} {'RELAY':>10} {'DIRECT':>10} {'DIFF':>10}")
print("-" * 60)
for r in results:
    loss = f"{r['loss_pct']}% loss"
    rl = r.get("relay_latency", {})
    dl = r.get("direct_latency", {})
    if "error" in rl or "error" in dl:
        print(f"  {loss:<18} {'p50':>8} {'FAIL':>10} {'FAIL':>10}")
        continue
    for metric in ["p50_ms", "p95_ms", "p99_ms", "mean_ms"]:
        rv = rl.get(metric, "-")
        dv = dl.get(metric, "-")
        label = metric.replace("_ms", "")
        if isinstance(rv, (int, float)) and isinstance(dv, (int, float)):
            diff = f"{rv - dv:+.1f}"
        else:
            diff = "-"
        print(f"  {loss:<18} {label:>8} {rv:>10} {dv:>10} {diff:>10}")
    loss = ""  # only show loss label once

# Throughput table
print(f"\n{'THROUGHPUT':<20} {'':>8} {'RELAY':>10} {'DIRECT':>10} {'RATIO':>10}")
print("-" * 60)
for r in results:
    loss = f"{r['loss_pct']}% loss"
    rt = r.get("relay_throughput", {})
    dt = r.get("direct_throughput", {})
    if "error" in rt or "error" in dt:
        print(f"  {loss:<18} {'msg/s':>8} {'FAIL':>10} {'FAIL':>10}")
        continue
    rr = rt.get("msgs_per_sec", "-")
    dr = dt.get("msgs_per_sec", "-")
    if isinstance(rr, (int, float)) and isinstance(dr, (int, float)) and dr > 0:
        ratio = f"{rr/dr:.2f}x"
    else:
        ratio = "-"
    rc = f"{rt.get('completed','?')}/{rt.get('rounds','?')}"
    dc = f"{dt.get('completed','?')}/{dt.get('rounds','?')}"
    print(f"  {loss:<18} {'msg/s':>8} {rr:>10} {dr:>10} {ratio:>10}")
    print(f"  {'':18} {'deliv':>8} {rc:>10} {dc:>10}")

print("\n" + "=" * 80)

# Dump raw JSON
print("\n[RAW RESULTS JSON]")
print(json.dumps(results, indent=2))

# Cleanup
run(node_b, "pkill -f 'echo_server\\|echo_relay' 2>/dev/null; true")
clear_netem(node_b)
clear_netem(node_a)
node_a.close()
node_b.close()
