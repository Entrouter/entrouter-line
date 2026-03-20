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
Coordinate benchmark across two remote VPS nodes.
Launches both sides simultaneously via SSH, collects results.

Usage:
  python coord_bench.py --rate-mbps 50 --duration 10
  python coord_bench.py --rate-mbps 0 --duration 10  (full blast)
"""
import argparse
import subprocess
import threading
import time
import sys

NODE_A_HOST = "root@YOUR_NODE_A_IP"
NODE_B_HOST = "root@YOUR_NODE_B_IP"
BENCH_CMD = "python3 /tmp/sync_bench.py"

def run_ssh(host, role, rate_mbps, duration, chunk_size, results, key):
    cmd = [
        "ssh", host,
        f"{BENCH_CMD} --role {role} --rate-mbps {rate_mbps} --duration {duration} --chunk-size {chunk_size}"
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=duration + 60)
        results[key] = {
            "stdout": proc.stdout,
            "stderr": proc.stderr,
            "returncode": proc.returncode,
        }
    except subprocess.TimeoutExpired:
        results[key] = {"stdout": "", "stderr": "TIMEOUT", "returncode": -1}

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--rate-mbps", type=float, default=0)
    p.add_argument("--duration", type=float, default=10)
    p.add_argument("--chunk-size", type=int, default=1024)
    args = p.parse_args()

    print(f"=== Coordinated Benchmark: {args.rate_mbps} Mbps, {args.duration}s, {args.chunk_size}B chunks ===")
    results = {}

    # Start both sides simultaneously
    t_a = threading.Thread(target=run_ssh, args=(NODE_A_HOST, "sender", args.rate_mbps, args.duration, args.chunk_size, results, "node_a"))
    t_b = threading.Thread(target=run_ssh, args=(NODE_B_HOST, "receiver", args.rate_mbps, args.duration, args.chunk_size, results, "node_b"))

    t_a.start()
    t_b.start()

    t_a.join()
    t_b.join()

    print("\n--- NODE A (sender) ---")
    if "node_a" in results:
        print(results["node_a"]["stdout"])
        if results["node_a"]["stderr"]:
            print(f"STDERR: {results['node_a']['stderr']}")
    else:
        print("NO RESULT")

    print("\n--- NODE B (receiver) ---")
    if "node_b" in results:
        print(results["node_b"]["stdout"])
        if results["node_b"]["stderr"]:
            print(f"STDERR: {results['node_b']['stderr']}")
    else:
        print("NO RESULT")

    # Parse SUMMARY lines
    for name, data in results.items():
        for line in data.get("stdout", "").split("\n"):
            if line.startswith("SUMMARY|"):
                print(f"\n{name.upper()}: {line}")

if __name__ == "__main__":
    main()
