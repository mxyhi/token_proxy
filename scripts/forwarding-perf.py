#!/usr/bin/env python3
"""本地 socket/SQLite 性能矩阵；只使用 Python 标准库，不接触供应商。"""
import argparse
import concurrent.futures
import hashlib
import http.client
import json
import math
import os
from pathlib import Path
import platform
import re
import signal
import sqlite3
import statistics
import subprocess
import tempfile
import threading
import time


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def request(port, barrier, slow_ms):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
    body = json.dumps({"model": "perf-model", "messages": [{"role": "user", "content": "hello"}], "stream": True})
    barrier.wait()
    started = time.perf_counter()
    connection.request("POST", "/v1/chat/completions", body, {"Content-Type": "application/json"})
    response = connection.getresponse()
    if response.status != 200:
        raise RuntimeError(f"HTTP {response.status}: {response.read(4096)!r}")
    first = response.read(1)
    first_ms = (time.perf_counter() - started) * 1000
    digest = hashlib.sha256(first)
    size = len(first)
    tail = first
    while True:
        chunk = response.read(16 * 1024)
        if not chunk:
            break
        digest.update(chunk)
        size += len(chunk)
        tail = (tail + chunk)[-64:]
        if slow_ms:
            time.sleep(slow_ms / 1000)
    connection.close()
    if not tail.endswith(b"data: [DONE]\n\n"):
        raise RuntimeError(f"missing final DONE: {tail!r}")
    return {"bytes": size, "sha256": digest.hexdigest(), "first_ms": first_ms, "total_ms": (time.perf_counter() - started) * 1000}


def run_case(args, detail, shape, concurrency, repeat):
    frames = 4 if shape == "normal" else 512
    slow_ms = 3 if shape == "slow" else 0
    case = f"{args.label}-{detail}-{shape}-c{concurrency}-r{repeat}"
    with tempfile.TemporaryDirectory(prefix="token-proxy-perf-") as tmp:
        tmp = Path(tmp)
        time_log = args.output / f"{case}.time.txt"
        server_log = args.output / f"{case}.server.txt"
        # time -l 的 max RSS/CPU 覆盖服务器（包括固定大小 mock），不含 Python 客户端。
        command = ["/usr/bin/time", "-l", str(args.binary.resolve()), str(tmp), detail, str(frames)]
        with server_log.open("w") as errors:
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors, text=True, start_new_session=True)
            ready_line = process.stdout.readline()
            if not ready_line:
                process.wait()
                raise RuntimeError(server_log.read_text())
            ready = json.loads(ready_line)
            samples = []
            stop = threading.Event()
            def sample():
                while not stop.is_set():
                    result = subprocess.run(["ps", "-o", "rss=", "-p", str(ready["pid"])], capture_output=True, text=True)
                    if result.stdout.strip():
                        samples.append(int(result.stdout.strip()) * 1024)
                    stop.wait(.1)
            monitor = threading.Thread(target=sample)
            monitor.start()
            try:
                # 相同预热负载，让 token estimator 等一次性初始化不混入时延分布。
                request(ready["port"], threading.Barrier(1), 0)
                time.sleep(.1)
                idle_rss = samples[-1] if samples else 0
                started = time.perf_counter()
                results = []
                with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
                    for _ in range(args.rounds):
                        barrier = threading.Barrier(concurrency)
                        futures = [executor.submit(request, ready["port"], barrier, slow_ms) for _ in range(concurrency)]
                        results.extend(future.result() for future in futures)
                wall = time.perf_counter() - started
                if any(result["bytes"] != ready["expected_bytes"] for result in results):
                    raise RuntimeError("forwarded byte count differs from mock")
                expected_count = len(results) + 1
                deadline = time.monotonic() + 30
                while True:
                    with sqlite3.connect(tmp / "data.db") as database:
                        count, failures, body_bytes = database.execute("SELECT count(*), sum(status != 200), sum(length(response_body)) FROM request_logs").fetchone()
                    if count == expected_count:
                        break
                    if time.monotonic() > deadline:
                        raise RuntimeError(f"SQLite drain timeout: {count}/{expected_count}")
                    time.sleep(.05)
                drain = time.perf_counter() - started - wall
                if failures:
                    raise RuntimeError(f"failed persisted requests: {failures}")
                chunk_bytes = 0
                with sqlite3.connect(tmp / "data.db") as database:
                    has_chunks = database.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='response_body_chunks'").fetchone()
                    if has_chunks:
                        chunk_bytes = database.execute("SELECT coalesce(sum(length(data)), 0) FROM response_body_chunks").fetchone()[0]
                        capture_errors = database.execute("SELECT count(*) FROM request_logs WHERE response_capture_error IS NOT NULL").fetchone()[0]
                        if capture_errors:
                            raise RuntimeError(f"detail capture errors: {capture_errors}")
                    expected_body_bytes = expected_count * ready["expected_bytes"] if detail == "on" else 0
                    if (body_bytes or 0) + chunk_bytes != expected_body_bytes:
                        raise RuntimeError(f"persisted detail bytes {(body_bytes or 0) + chunk_bytes} != {expected_body_bytes}")
                    if detail == "on":
                        expected_hash = results[0]["sha256"]
                        for log_id, inline_body in database.execute("SELECT id, response_body FROM request_logs"):
                            digest = hashlib.sha256()
                            if inline_body is not None:
                                digest.update(inline_body.encode())
                            elif has_chunks:
                                for (chunk,) in database.execute("SELECT data FROM response_body_chunks WHERE log_id=? ORDER BY ordinal", (log_id,)):
                                    digest.update(chunk)
                            if digest.hexdigest() != expected_hash:
                                raise RuntimeError(f"persisted body hash mismatch: log {log_id}")
                process.stdin.write("stop\n")
                process.stdin.flush()
                process.wait(timeout=20)
                if process.returncode:
                    raise RuntimeError(f"server exit {process.returncode}")
            finally:
                stop.set()
                monitor.join()
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        metrics = server_log.read_text()
        time_log.write_text(metrics)
        match = re.search(r"([\d.]+) real\s+([\d.]+) user\s+([\d.]+) sys", metrics)
        rss = re.search(r"(\d+)\s+maximum resident set size", metrics)
        cpu = sum(map(float, match.group(2, 3))) if match else None
        row = {"label": args.label, "detail": detail, "shape": shape, "concurrency": concurrency, "repeat": repeat, "requests": len(results), "frames": frames, "expected_bytes": ready["expected_bytes"], "wall_s": wall, "rps": len(results) / wall, "mib_per_s": sum(r["bytes"] for r in results) / wall / 1024**2, "first_p50_ms": statistics.median(r["first_ms"] for r in results), "first_p95_ms": percentile([r["first_ms"] for r in results], .95), "latency_p95_ms": percentile([r["total_ms"] for r in results], .95), "idle_rss_mib": idle_rss / 1024**2, "sampled_peak_rss_mib": max(samples, default=0) / 1024**2, "process_peak_rss_mib": int(rss.group(1)) / 1024**2 if rss else None, "process_cpu_s": cpu, "sqlite_drain_s": drain, "sqlite_rows": count, "sqlite_inline_body_bytes": body_bytes or 0, "sqlite_chunk_body_bytes": chunk_bytes, "failures": failures or 0}
        print(json.dumps(row), flush=True)
        with (args.output / f"{args.label}.jsonl").open("a") as output:
            output.write(json.dumps(row) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--concurrency", type=int, nargs="+", default=[1, 8, 32])
    parser.add_argument("--shapes", nargs="+", default=["normal", "long", "slow"])
    parser.add_argument("--details", nargs="+", default=["off", "on"])
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("当前 /usr/bin/time -l 解析仅适用于 macOS")
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / f"{args.label}-metadata.json").write_text(json.dumps({"platform": platform.platform(), "binary": str(args.binary.resolve()), "sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(), "rounds": args.rounds, "repeats": args.repeats, "command": " ".join(os.sys.argv), "cpu_count": os.cpu_count()}, indent=2))
    for repeat in range(1, args.repeats + 1):
        for detail in args.details:
            for shape in args.shapes:
                for concurrency in args.concurrency:
                    run_case(args, detail, shape, concurrency, repeat)


if __name__ == "__main__":
    main()
