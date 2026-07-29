#!/usr/bin/env python3
"""Reproducible local Kilovolt benchmark using only Python's standard library."""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import http.client
import json
import os
import pathlib
import platform
import signal
import socket
import statistics
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from typing import Any


ROOT = pathlib.Path(__file__).resolve().parents[2]
BINARY = ROOT / "target" / "release" / "kilovolt"


def command_output(command: list[str], default: str = "unavailable") -> str:
    try:
        return subprocess.check_output(
            command, cwd=ROOT, text=True, stderr=subprocess.DEVNULL
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return default


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def total_memory_bytes() -> int | None:
    if sys.platform == "darwin":
        value = command_output(["sysctl", "-n", "hw.memsize"], "")
        return int(value) if value.isdigit() else None
    try:
        for line in pathlib.Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1]) * 1024
    except OSError:
        pass
    return None


def cpu_model() -> str:
    if sys.platform == "darwin":
        return command_output(["sysctl", "-n", "machdep.cpu.brand_string"])
    try:
        for line in pathlib.Path("/proc/cpuinfo").read_text().splitlines():
            if line.lower().startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or "unavailable"


def percentile(values: list[float], quantile: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int(round((len(ordered) - 1) * quantile))))
    return ordered[index]


@dataclass
class Sample:
    status: int
    ttfb_ms: float
    total_ms: float
    bytes_read: int


class ProcessSampler:
    def __init__(self, pid: int) -> None:
        self.pid = pid
        self.rss_kb: list[int] = []
        self.cpu_percent: list[float] = []
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        while not self._stop.is_set():
            output = command_output(
                ["ps", "-o", "rss=", "-o", "%cpu=", "-p", str(self.pid)], ""
            )
            fields = output.split()
            if len(fields) >= 2:
                try:
                    self.rss_kb.append(int(fields[0]))
                    self.cpu_percent.append(float(fields[1]))
                except ValueError:
                    pass
            self._stop.wait(0.05)

    def __enter__(self) -> "ProcessSampler":
        self._thread.start()
        return self

    def __exit__(self, *_: object) -> None:
        self._stop.set()
        self._thread.join(timeout=1)


class KilovoltProcess:
    def __init__(self, project_budget: str, user_budget: str) -> None:
        self.port = free_port()
        environment = os.environ.copy()
        environment.update(
            {
                "BIND_ADDR": "127.0.0.1",
                "KILOVOLT_PORT": str(self.port),
                "KILOVOLT_PROJECT_BUDGET": project_budget,
                "KILOVOLT_DEFAULT_BUDGET": user_budget,
                "KILOVOLT_DASHBOARD_TOKEN": "benchmark-only",
                "KILOVOLT_TELEMETRY_ENABLED": "false",
                "KILOVOLT_MAX_REQUEST_BODY_BYTES": str(1024 * 1024),
                "KILOVOLT_MAX_UPSTREAM_BODY_BYTES": str(4 * 1024 * 1024),
                "KILOVOLT_MAX_SSE_FRAME_BYTES": str(256 * 1024),
                "KILOVOLT_ENABLE_MOCK_UPSTREAM": "true",
                "KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS": "4096",
                "RUST_LOG": "kilovolt=error",
            }
        )
        self.process = subprocess.Popen(
            [str(BINARY)],
            cwd=ROOT,
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self._wait_until_ready()

    def _wait_until_ready(self) -> None:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError("Kilovolt exited before becoming healthy")
            try:
                connection = http.client.HTTPConnection(
                    "127.0.0.1", self.port, timeout=0.5
                )
                connection.request("GET", "/health")
                if connection.getresponse().status == 200:
                    connection.close()
                    return
            except OSError:
                time.sleep(0.05)
        raise RuntimeError("Kilovolt did not become healthy within 10 seconds")

    def stop(self) -> None:
        if self.process.poll() is not None:
            return
        self.process.send_signal(signal.SIGTERM)
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=2)

    def __enter__(self) -> "KilovoltProcess":
        return self

    def __exit__(self, *_: object) -> None:
        self.stop()


def request_once(
    port: int,
    route: str,
    payload: bytes,
    event_count: int,
    user_id: str = "benchmark-user",
) -> Sample:
    headers = {
        "Authorization": "Bearer benchmark-key",
        "Content-Type": "application/json",
        "X-User-ID": user_id,
        "X-Mock-Events": str(event_count),
    }
    if route == "/v1/chat/completions":
        headers["X-Mock-Upstream"] = "true"
    started = time.perf_counter()
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    connection.request("POST", route, body=payload, headers=headers)
    response = connection.getresponse()
    first = response.read(1)
    first_byte = time.perf_counter()
    remainder = response.read()
    finished = time.perf_counter()
    connection.close()
    return Sample(
        status=response.status,
        ttfb_ms=(first_byte - started) * 1000,
        total_ms=(finished - started) * 1000,
        bytes_read=len(first) + len(remainder),
    )


def workload(
    process: KilovoltProcess,
    name: str,
    route: str,
    payload: bytes,
    streaming: bool,
    event_count: int,
    concurrency: int,
    warmup_requests: int,
    measured_requests: int,
    unique_users: bool = False,
) -> dict[str, Any]:
    for index in range(warmup_requests):
        request_once(
            process.port,
            route,
            payload,
            event_count,
            f"warmup-user-{index}" if unique_users else "benchmark-user",
        )

    with ProcessSampler(process.process.pid) as sampler:
        started = time.perf_counter()
        with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
            samples = list(
                pool.map(
                    lambda index: request_once(
                        process.port,
                        route,
                        payload,
                        event_count,
                        f"measured-user-{index}" if unique_users else "benchmark-user",
                    ),
                    range(measured_requests),
                )
            )
        elapsed = time.perf_counter() - started

    ttfb = [sample.ttfb_ms for sample in samples]
    total = [sample.total_ms for sample in samples]
    status_counts: dict[str, int] = {}
    for sample in samples:
        key = str(sample.status)
        status_counts[key] = status_counts.get(key, 0) + 1
    return {
        "name": name,
        "route": route,
        "streaming": streaming,
        "event_count": event_count,
        "concurrency": concurrency,
        "warmup_requests": warmup_requests,
        "measured_requests": measured_requests,
        "elapsed_seconds": elapsed,
        "throughput_requests_per_second": measured_requests / elapsed,
        "ttfb_ms": {
            "p50": percentile(ttfb, 0.50),
            "p95": percentile(ttfb, 0.95),
            "p99": percentile(ttfb, 0.99),
        },
        "end_to_end_ms": {
            "p50": percentile(total, 0.50),
            "p95": percentile(total, 0.95),
            "p99": percentile(total, 0.99),
        },
        "response_bytes": {
            "min": min(sample.bytes_read for sample in samples),
            "max": max(sample.bytes_read for sample in samples),
        },
        "status_counts": status_counts,
        "rss_kb": {
            "sample_count": len(sampler.rss_kb),
            "active_median": statistics.median(sampler.rss_kb)
            if sampler.rss_kb
            else None,
            "peak": max(sampler.rss_kb) if sampler.rss_kb else None,
        },
        "cpu_percent": {
            "sample_count": len(sampler.cpu_percent),
            "mean": statistics.fmean(sampler.cpu_percent)
            if sampler.cpu_percent
            else None,
            "peak": max(sampler.cpu_percent) if sampler.cpu_percent else None,
        },
    }


def prompt_payload(streaming: bool, prompt_bytes: int = 0) -> bytes:
    content = "p" * prompt_bytes if prompt_bytes else "deterministic prompt"
    return json.dumps(
        {
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": content}],
            "stream": streaming,
        },
        separators=(",", ":"),
    ).encode()


def run_suite(smoke: bool) -> dict[str, Any]:
    if not BINARY.exists():
        raise RuntimeError("target/release/kilovolt is missing; run cargo build --release")
    levels = [1, 5] if smoke else [1, 10, 50, 100]
    results: list[dict[str, Any]] = []
    idle_rss_kb: int | None = None

    with KilovoltProcess("1000", "1000") as process:
        time.sleep(0.2)
        idle = command_output(
            ["ps", "-o", "rss=", "-p", str(process.process.pid)], ""
        ).strip()
        idle_rss_kb = int(idle) if idle.isdigit() else None

        for streaming in (True, False):
            payload = prompt_payload(streaming)
            for concurrency in levels:
                measured = max(5 if smoke else 30, concurrency * (1 if smoke else 2))
                warmup = min(3 if smoke else 10, measured)
                for route, label in (
                    ("/mock/v1/chat/completions", "direct"),
                    ("/v1/chat/completions", "proxied"),
                ):
                    results.append(
                        workload(
                            process,
                            f"{label}-{'stream' if streaming else 'json'}-c{concurrency}",
                            route,
                            payload,
                            streaming,
                            4,
                            concurrency,
                            warmup,
                            measured,
                        )
                    )

        long_events = 100 if smoke else 5000
        for route, label in (
            ("/mock/v1/chat/completions", "direct"),
            ("/v1/chat/completions", "proxied"),
        ):
            results.append(
                workload(
                    process,
                    f"{label}-long-stream",
                    route,
                    prompt_payload(True),
                    True,
                    long_events,
                    1,
                    1,
                    2 if smoke else 3,
                )
            )

        large_prompt_bytes = 64 * 1024 if smoke else 512 * 1024
        for route, label in (
            ("/mock/v1/chat/completions", "direct"),
            ("/v1/chat/completions", "proxied"),
        ):
            results.append(
                workload(
                    process,
                    f"{label}-large-prompt",
                    route,
                    prompt_payload(False, large_prompt_bytes),
                    False,
                    4,
                    1,
                    1,
                    3 if smoke else 10,
                )
            )

    with KilovoltProcess("0", "0") as rejection_process:
        results.append(
            workload(
                rejection_process,
                "preflight-budget-rejection",
                "/v1/chat/completions",
                prompt_payload(True),
                True,
                4,
                10 if not smoke else 2,
                2,
                100 if not smoke else 10,
            )
        )

    with KilovoltProcess("1", "0.000005") as cutoff_process:
        results.append(
            workload(
                cutoff_process,
                "mid-stream-budget-cutoff",
                "/v1/chat/completions",
                prompt_payload(True),
                True,
                100,
                1,
                1,
                3 if smoke else 10,
                True,
            )
        )

    paired_overhead: list[dict[str, Any]] = []
    by_name = {entry["name"]: entry for entry in results}
    for name, direct in by_name.items():
        if not name.startswith("direct-"):
            continue
        proxied = by_name.get(name.replace("direct-", "proxied-", 1))
        if proxied is None:
            continue
        paired_overhead.append(
            {
                "workload": name.removeprefix("direct-"),
                "ttfb_p50_added_ms": proxied["ttfb_ms"]["p50"]
                - direct["ttfb_ms"]["p50"],
                "end_to_end_p50_added_ms": proxied["end_to_end_ms"]["p50"]
                - direct["end_to_end_ms"]["p50"],
                "throughput_difference_requests_per_second": proxied[
                    "throughput_requests_per_second"
                ]
                - direct["throughput_requests_per_second"],
            }
        )

    return {
        "schema_version": 1,
        "timestamp_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "kilovolt": {
            "commit_sha": command_output(["git", "rev-parse", "HEAD"]),
            "working_tree_dirty": bool(command_output(["git", "status", "--porcelain"], "")),
            "build_profile": "release",
            "rust_version": command_output(["rustc", "--version"]),
        },
        "environment": {
            "operating_system": platform.platform(),
            "architecture": platform.machine(),
            "cpu_model": cpu_model(),
            "logical_cpu_count": os.cpu_count(),
            "total_memory_bytes": total_memory_bytes(),
        },
        "configuration": {
            "smoke": smoke,
            "concurrency_levels": levels,
            "stream_events": 4,
            "long_stream_events": 100 if smoke else 5000,
            "large_prompt_bytes": 64 * 1024 if smoke else 512 * 1024,
            "telemetry_enabled": False,
            "request_body_limit_bytes": 1024 * 1024,
            "upstream_body_limit_bytes": 4 * 1024 * 1024,
            "sse_frame_limit_bytes": 256 * 1024,
        },
        "commands": {
            "build": "cargo build --release",
            "benchmark": "python3 " + " ".join(sys.argv),
        },
        "idle_rss_kb": idle_rss_kb,
        "results": results,
        "paired_proxy_overhead": paired_overhead,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--smoke", action="store_true", help="run a short validation suite")
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        help="raw JSON path (default: benchmark-results/<UTC timestamp>.json)",
    )
    arguments = parser.parse_args()
    timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = arguments.output or ROOT / "benchmark-results" / f"{timestamp}.json"
    output.parent.mkdir(parents=True, exist_ok=True)

    results = run_suite(arguments.smoke)
    output.write_text(json.dumps(results, indent=2) + "\n")
    print(f"raw results: {output}")
    print(f"idle RSS: {results['idle_rss_kb']} KiB")
    for overhead in results["paired_proxy_overhead"]:
        print(
            f"{overhead['workload']}: "
            f"p50 added TTFB {overhead['ttfb_p50_added_ms']:.3f} ms, "
            f"p50 added total {overhead['end_to_end_p50_added_ms']:.3f} ms"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
