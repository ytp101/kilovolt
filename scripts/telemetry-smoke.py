#!/usr/bin/env python3
"""Verify telemetry opt-in behavior against a local receiver."""

from __future__ import annotations

import http.server
import json
import os
import pathlib
import signal
import socket
import socketserver
import subprocess
import tempfile
import threading
import time
import urllib.request


ROOT = pathlib.Path(__file__).resolve().parents[1]
BINARY = ROOT / "target" / "release" / "kilovolt"


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


class Receiver(http.server.ThreadingHTTPServer):
    payloads: list[dict[str, object]]

    def server_bind(self) -> None:
        # Avoid HTTPServer's reverse-DNS lookup, which can stall offline runs.
        socketserver.TCPServer.server_bind(self)
        self.server_name = str(self.server_address[0])
        self.server_port = int(self.server_address[1])


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802 - stdlib callback name
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length))
        self.server.payloads.append(payload)  # type: ignore[attr-defined]
        response = json.dumps(
            {
                "latest_version": "1.3.1",
                "update_available": False,
                "message": "local telemetry smoke receiver",
            }
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *_: object) -> None:
        return


def wait_for_health(port: int) -> None:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(
                f"http://127.0.0.1:{port}/health", timeout=0.5
            ) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("Kilovolt did not become healthy")


def send_mock_request(port: int) -> None:
    payload = json.dumps(
        {
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "telemetry smoke"}],
            "stream": False,
        }
    ).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions",
        data=payload,
        headers={
            "Authorization": "Bearer mock-key",
            "Content-Type": "application/json",
            "X-User-ID": "telemetry-smoke-user",
            "X-Mock-Upstream": "true",
        },
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=5) as response:
        if response.status != 200:
            raise RuntimeError(f"mock request returned {response.status}")
        response.read()


def run_instance(enabled: bool, endpoint: str) -> None:
    port = free_port()
    environment = os.environ.copy()
    environment.update(
        {
            "BIND_ADDR": "127.0.0.1",
            "KILOVOLT_PORT": str(port),
            "KILOVOLT_PROJECT_BUDGET": "10",
            "KILOVOLT_DEFAULT_BUDGET": "5",
            "KILOVOLT_TELEMETRY_ENABLED": "true" if enabled else "false",
            "KILOVOLT_TELEMETRY_URL": endpoint,
            "RUST_LOG": "kilovolt=error",
        }
    )
    with tempfile.TemporaryDirectory() as directory:
        process = subprocess.Popen(
            [str(BINARY)],
            cwd=directory,
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            wait_for_health(port)
            send_mock_request(port)
            time.sleep(0.3)
        finally:
            process.send_signal(signal.SIGTERM)
            process.wait(timeout=5)


def main() -> int:
    if not BINARY.exists():
        raise RuntimeError("target/release/kilovolt is missing; run cargo build --release")
    receiver = Receiver(("127.0.0.1", 0), Handler)
    receiver.payloads = []
    thread = threading.Thread(target=receiver.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{receiver.server_port}/telemetry"
    try:
        run_instance(False, endpoint)
        if receiver.payloads:
            raise AssertionError("telemetry-disabled instance contacted the receiver")

        run_instance(True, endpoint)
        types = [payload.get("type") for payload in receiver.payloads]
        if "startup" not in types or "tsum_update" not in types:
            raise AssertionError(f"missing enabled telemetry events: {types}")
        startup = next(payload for payload in receiver.payloads if payload["type"] == "startup")
        if set(startup) != {
            "type",
            "client_hash",
            "version",
            "is_docker",
            "os",
            "arch",
        }:
            raise AssertionError(f"unexpected startup fields: {sorted(startup)}")
        tsum = next(
            payload for payload in receiver.payloads if payload["type"] == "tsum_update"
        )
        if set(tsum) != {"type", "client_hash", "cost"}:
            raise AssertionError(f"unexpected request fields: {sorted(tsum)}")
    finally:
        receiver.shutdown()
        receiver.server_close()
        thread.join(timeout=2)

    print("telemetry disabled/enabled local receiver checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
