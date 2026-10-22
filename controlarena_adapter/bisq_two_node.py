"""Three-process isolated Bisq 2 developer network: seed + Alice/Bob API nodes.

The upstream local-3node script validates the launcher conventions.
Requires a separately built Bisq2 checkout (installDist binaries) and POSIX.
The runner never disables API session authentication and never sends trades.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import socket
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

from .bisq_pairing import PairingError, PairedSession, pair_node, read_pairing_file
from .bisq_correlate import verify_correlation
from .bitcoin_regtest import BitcoinCoreRPC, RegtestError


class NodeError(RuntimeError):
    pass


@dataclass(frozen=True)
class NodeSpec:
    name: str
    program: Path
    data_dir: Path
    p2p_port: int
    api_port: int | None
    seed_port: int

    def java_opts(self) -> str:
        # This follows upstream scripts/local-3node.sh.
        options = [
            "-Dapplication.network.supportedTransportTypes.0=CLEAR",
            f"-Dapplication.network.configByTransportType.clear.defaultNodePort={self.p2p_port}",
            f"-Dapplication.network.seedAddressByTransportType.clear.0=127.0.0.1:{self.seed_port}",
            f"-Dapplication.network.seedAddressByTransportType.clear.1=127.0.0.1:{self.seed_port}",
        ]
        if self.api_port:
            options.extend([
                "-Dapplication.api.accessTransportType=CLEAR",
                "-Dapplication.api.server.restEnabled=true",
                "-Dapplication.api.server.websocketEnabled=true",
                "-Dapplication.api.server.bind.host=127.0.0.1",
                f"-Dapplication.api.server.bind.port={self.api_port}",
                "-Dapplication.api.pairing.writePairingQrCodeToDisk=true",
            ])
        return " ".join(options)


def plan(bisq_root: Path, data_root: Path, port_base: int = 18500) -> tuple[NodeSpec, ...]:
    if not 1024 <= port_base <= 64000:
        raise NodeError("Port base must be between 1024 and 64000")
    source = Path(bisq_root).expanduser().resolve()
    data = Path(data_root).expanduser().resolve()
    seed = source / "apps/seed-node-app/build/install/seed-node-app/bin/seed-node-app"
    api = source / "apps/api-app/build/install/api-app/bin/api-app"
    return (
        NodeSpec("seed", seed, data / "seed", port_base, None, port_base),
        NodeSpec("alice", api, data / "alice", port_base + 1, port_base + 100, port_base),
        NodeSpec("bob", api, data / "bob", port_base + 2, port_base + 101, port_base),
    )


def _port_free(port: int) -> bool:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", port))
        return True
    except OSError:
        return False
    finally:
        sock.close()


class BisqPair:
    def __init__(self, nodes: tuple[NodeSpec, ...]) -> None:
        if len(nodes) != 3 or [n.name for n in nodes] != ["seed", "alice", "bob"]:
            raise NodeError("Expected one seed and exactly two API nodes")
        self.nodes = nodes
        self.processes: list[subprocess.Popen] = []
        self.log_files: list[object] = []

    def __enter__(self) -> "BisqPair":
        if os.name != "posix":
            raise NodeError("Two-node launcher requires Linux or macOS POSIX process groups")
        ports = [n.p2p_port for n in self.nodes] + [
            n.api_port for n in self.nodes if n.api_port is not None
        ]
        if not all(_port_free(port) for port in ports):
            raise NodeError("One or more Bisq test ports are already bound")
        for node in self.nodes:
            if not node.program.is_file() or not os.access(node.program, os.X_OK):
                raise NodeError(f"Bisq installDist binary missing: {node.program}")
            if node.data_dir.exists() and list(node.data_dir.iterdir()):
                raise NodeError("Refusing to reuse a nonempty Bisq data directory")
        try:
            for node in self.nodes:
                node.data_dir.mkdir(parents=True, mode=0o700)
                node.data_dir.chmod(0o700)
                log = (node.data_dir / "launcher.log").open("w", encoding="utf-8")
                os.chmod(log.name, 0o600)
                self.log_files.append(log)
                env = os.environ.copy()
                env["JAVA_OPTS"] = node.java_opts()
                proc = subprocess.Popen(
                    [
                        str(node.program),
                        f"--app-name=rtg_{node.name}",
                        f"--data-dir={node.data_dir}",
                    ],
                    env=env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    cwd=node.program.parents[6],
                    start_new_session=True,
                )
                self.processes.append(proc)
                # Permit local seed to bind before API clients begin bootstrap.
                if node.name == "seed":
                    self._wait_port(node.p2p_port, 45)
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def _wait_port(self, port: int, timeout: float) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if any(p.poll() is not None for p in self.processes):
                raise NodeError("Bisq process exited before local API or P2P readiness")
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                    return
            except OSError:
                time.sleep(0.2)
        raise NodeError(f"Local Bisq port {port} did not become ready")

    def wait_for_apis(self, timeout: float = 90) -> None:
        opener = build_opener(ProxyHandler({}))
        for node in self.nodes[1:]:
            assert node.api_port is not None
            self._wait_port(node.api_port, timeout)
            # Check an actual protected API resource. HTTP 401/403 shows
            # the server is running with authentication, while 200 indicates
            # local credentials are unnecessary for this resource.
            endpoint = f"http://127.0.0.1:{node.api_port}/api/v1/access/clients"
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if any(proc.poll() is not None for proc in self.processes):
                    raise NodeError("Bisq process exited before API readiness")
                try:
                    with opener.open(Request(endpoint, method="GET"), timeout=4) as response:
                        if response.status == 200:
                            break
                except HTTPError as exc:
                    if exc.code in (401, 403):
                        break
                    # Route registration can trail the initial socket bind.
                    if exc.code not in (404, 500, 503):
                        raise NodeError(f"Bisq API returned unexpected HTTP {exc.code}") from exc
                except URLError:
                    pass
                time.sleep(0.35)
            else:
                raise NodeError(f"Bisq API on {node.api_port} did not become HTTP ready")

    def pair(self, timeout: float = 60) -> tuple[PairedSession, PairedSession]:
        credentials: list[PairedSession] = []
        for node in self.nodes[1:]:
            assert node.api_port is not None
            path = node.data_dir / "pairing_qr_code.txt"
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline and not path.exists():
                time.sleep(0.2)
            if not path.exists():
                raise NodeError(f"Bisq {node.name} did not write pairing QR file")
            # These are freshly generated private directories owned by runner.
            # The QR may inherit a permissive umask, so tighten file mode.
            path.chmod(0o600)
            ticket = read_pairing_file(path, node.api_port)
            credentials.append(pair_node(ticket, node.api_port, f"rtg_{node.name}"))
        return credentials[0], credentials[1]

    def __exit__(self, exception_type, exception, traceback) -> None:
        for process in reversed(self.processes):
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        for process in reversed(self.processes):
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
        for log in self.log_files:
            log.close()
        self.processes.clear()
        self.log_files.clear()


def main() -> None:
    parser = argparse.ArgumentParser(description="Isolated Bisq2 seed + Alice/Bob API nodes")
    parser.add_argument("--bisq-root", required=True, type=Path)
    parser.add_argument("--port-base", type=int, default=18500)
    parser.add_argument("--plan", action="store_true", help="Show plan only; start nothing")
    parser.add_argument("--launch", action="store_true", help="Explicitly start all three apps")
    parser.add_argument("--hold", type=int, default=30)
    parser.add_argument("--trade-id", help="Optional existing Bisq Easy trade ID on both nodes")
    parser.add_argument("--probe-subscriptions", action="store_true", help="Verify both trade-state WebSocket permissions without a trade")
    parser.add_argument("--expected-sats", type=int, default=100_000)
    parser.add_argument("--trade-wait", type=int, default=0, help="Seconds to await matching Bisq trade state")
    parser.add_argument("--txid", help="Optional existing confirmed Bitcoin Core regtest TXID")
    parser.add_argument("--export-sessions", action="store_true", help="Write private credentials in temporary workdir")
    args = parser.parse_args()
    if args.plan == args.launch:
        parser.error("Select exactly one of --plan or --launch")
    if not 0 <= args.hold <= 3600:
        parser.error("--hold must be 0..3600 seconds")
    if args.txid and not args.trade_id:
        parser.error("--txid requires --trade-id")
    if not 0 <= args.trade_wait <= 600:
        parser.error("--trade-wait must be 0..600 seconds")
    from .bisq_ws import BisqStreamError, snapshot, reconcile

    try:
        with tempfile.TemporaryDirectory(prefix="rtg-bisq-pair-") as root:
            nodes = plan(args.bisq_root, Path(root), args.port_base)
            public = [
                {
                    "name": node.name,
                    "p2p_port": node.p2p_port,
                    "api_port": node.api_port,
                    "binary_found": node.program.is_file(),
                }
                for node in nodes
            ]
            if args.plan:
                print(json.dumps({"kind": "bisq_pair_plan", "nodes": public}, sort_keys=True))
                return
            with BisqPair(nodes) as launched:
                launched.wait_for_apis()
                alice, bob = launched.pair()
                if args.probe_subscriptions:
                    sentinel = "rtg_subscription_probe_001"
                    for session in (alice, bob):
                        view = snapshot(session, sentinel, args.expected_sats, listen_seconds=0)
                        if view.state is not None:
                            raise NodeError("Unexpected synthetic probe trade in Bisq")
                if args.export_sessions:
                    session_dir = Path(root) / "sessions"
                    session_dir.mkdir(mode=0o700)
                    for label, session in (("alice", alice), ("bob", bob)):
                        destination = session_dir / f"{label}.json"
                        payload = json.dumps({
                            "client_id": session.client_id,
                            "client_secret": session.client_secret,
                            "session_id": session.session_id,
                            "websocket_url": session.websocket_url,
                        })
                        file_descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                        with os.fdopen(file_descriptor, "w", encoding="utf-8") as stream:
                            stream.write(payload)
                print(json.dumps({
                    "kind": "bisq_pair_started",
                    "node_count": 3,
                    "paired_client_count": 2,
                    "websocket_subscriptions_verified": bool(args.probe_subscriptions),
                    "nodes": public,
                    "temporary_workspace": root if args.export_sessions else None,
                }, sort_keys=True))
                if args.trade_id:
                    deadline = time.monotonic() + args.trade_wait
                    while True:
                        a = snapshot(alice, args.trade_id, args.expected_sats)
                        b = snapshot(bob, args.trade_id, args.expected_sats)
                        if a.state is not None and b.state is not None:
                            break
                        if time.monotonic() >= deadline:
                            raise NodeError("Trade ID not observed on both Bisq API nodes")
                        time.sleep(1)
                    report = reconcile(a, b, args.expected_sats)
                    if args.txid:
                        core = BitcoinCoreRPC(
                            user=os.getenv("P2P_REGTEST_RPC_USER"),
                            password=os.getenv("P2P_REGTEST_RPC_PASSWORD"),
                        )
                        report = verify_correlation(core, a, b, args.txid, args.expected_sats)
                    print(json.dumps(report, sort_keys=True))
                if args.hold:
                    time.sleep(args.hold)
    except (NodeError, PairingError, BisqStreamError, RegtestError, OSError) as exc:
        parser.exit(1, f"Two-node Bisq harness stopped: {exc}\n")


if __name__ == "__main__":
    main()
