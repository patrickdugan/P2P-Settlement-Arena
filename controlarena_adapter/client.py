"""Allowlisted stateless Rust replay client.

The trusted wrapper owns event history and calls the synthetic Rust binary
using an argument vector and never a shell. The exposed agent-tool surface
cannot create a bank-payment event or alter the trusted fixture configuration.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Sequence

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BINARY = ROOT / "target" / "debug" / "settlement-arena"
MAX_ACTIONS = 32
CASES = frozenset(("paid", "delayed", "unpaid", "poisoned-invoice", "reversible"))
GUARDS = frozenset(("strict", "legacy"))
ACTIONS = frozenset(("observe", "advance", "confirm", "send", "decline", "trust-claim"))


class GatewayError(RuntimeError):
    """The deterministic Rust model refused or failed the replay."""


class ArenaGateway:
    def __init__(self, binary_path: str | None = None, timeout: float = 10.0) -> None:
        self.binary_path = binary_path
        self.timeout = timeout

    def _command(self) -> list[str]:
        path = self.binary_path or os.environ.get("P2P_ARENA_BINARY")
        if path:
            binary = Path(path).expanduser().resolve()
            if not binary.is_file():
                raise GatewayError(f"Configured settlement binary does not exist: {binary}")
            return [str(binary)]
        if DEFAULT_BINARY.is_file():
            return [str(DEFAULT_BINARY)]
        # Development fallback; run cargo in the trusted host, never with user args.
        return ["cargo", "run", "--quiet", "--"]

    def replay(
        self, case: str, guard: str = "strict", actions: Sequence[str] = ()
    ) -> dict[str, object]:
        if case not in CASES:
            raise GatewayError(f"Unknown control case: {case}")
        if guard not in GUARDS:
            raise GatewayError(f"Unknown control guard: {guard}")
        history = list(actions)
        if len(history) > MAX_ACTIONS or any(action not in ACTIONS for action in history):
            raise GatewayError("Invalid or excessive operator action history")
        serialized = ",".join(history)
        if len(serialized) > 512:
            raise GatewayError("Action history exceeds Rust replay cap")
        cmd = [*self._command(), "control", case, guard, serialized]
        try:
            result = subprocess.run(
                cmd,
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=self.timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise GatewayError(f"Unable to run Rust control engine: {error}") from error
        if result.returncode != 0:
            raise GatewayError(
                f"Rust engine exited {result.returncode}: {result.stderr.strip()[:400]}"
            )
        try:
            report = json.loads(result.stdout)
        except (json.JSONDecodeError, UnicodeError) as error:
            raise GatewayError("Rust engine returned non-JSON output") from error
        if (
            not isinstance(report, dict)
            or report.get("kind") != "control_state"
            or report.get("protocol") != "bisq-easy"
            or report.get("case") != case
            or report.get("guard") != guard
            or report.get("journal_valid") is not True
            or report.get("actions_attempted") != len(history)
        ):
            raise GatewayError("Rust engine returned an inconsistent state record")
        if history and report.get("last_action") != history[-1]:
            raise GatewayError("Rust engine returned the wrong final action")
        return report
