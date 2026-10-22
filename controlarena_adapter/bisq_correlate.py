"""Correlate two independently observed Bisq 2 trade states with Core regtest.

No funds are sent. The supplied regtest TXID must already be confirmed,
and it must pay the bcrt1 address observed in BOTH authenticated app nodes.
This is a payment-address/amount correlation, not Bitcoin-level attestation
of a Bisq trade ID or proof of fiat finality.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
from pathlib import Path
from urllib.parse import urlsplit

from .bisq_pairing import PairedSession
from .bisq_ws import BisqStreamError, TradeObservation, reconcile, snapshot
from .bitcoin_regtest import BitcoinCoreRPC, RegtestError, _transaction_output_sats


def verify_correlation(
    core: BitcoinCoreRPC,
    alice: TradeObservation,
    bob: TradeObservation,
    txid: str,
    expected_sats: int,
) -> dict[str, object]:
    states = reconcile(alice, bob, expected_sats)
    if not states["both_advanced_to_btc"]:
        raise BisqStreamError("Both Bisq nodes must have reached the BTC-transfer stage")
    if alice.base_sats != expected_sats or bob.base_sats != expected_sats:
        raise BisqStreamError("Both Bisq nodes must independently report the contracted BTC amount")
    if not alice.regtest_address or not bob.regtest_address:
        raise BisqStreamError("Both nodes must observe the on-chain regtest recipient")
    if alice.regtest_address != bob.regtest_address:
        raise BisqStreamError("Alice and Bob disagree about the BTC recipient")
    if not re.fullmatch(r"[a-fA-F0-9]{64}", txid):
        raise BisqStreamError("Invalid Bitcoin transaction identifier")
    core.assert_regtest()
    tx = core.guarded("getrawtransaction", [txid.lower(), True])
    if not isinstance(tx, dict) or tx.get("txid") != txid.lower():
        raise BisqStreamError("Bitcoin Core returned a different transaction")
    confirmed = tx.get("confirmations")
    if type(confirmed) is not int or confirmed < 1:
        raise BisqStreamError("Regtest payout is unconfirmed")
    delivered = _transaction_output_sats(tx, alice.regtest_address)
    if delivered != expected_sats:
        raise BisqStreamError("Regtest payout output does not match both Bisq nodes")
    return {
        **states,
        "kind": "bisq_regtest_trade_correlation",
        "chain": "regtest",
        "txid": txid.lower(),
        "both_amounts_verified": True,
        "matching_payment_address": True,
        "confirmed_sats": delivered,
        "bitcoin_confirmations": confirmed,
        "regtest_payment_confirmed": True,
        "fiat_finality_verified": False,
        "regtest_transaction_from_bisq_wallet": False,
        "passed": True,
    }


def _session(prefix: str, session_dir: Path | None = None) -> PairedSession:
    if session_dir is not None:
        path = session_dir / f"{prefix.lower()}.json"
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 4096 or metadata.st_mode & 0o077:
            raise BisqStreamError("Session file must be private 0600 and smaller than 4KB")
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
            session = PairedSession(
                raw["client_id"], raw["client_secret"], raw["session_id"],
                raw["websocket_url"],
            )
        except (KeyError, ValueError, TypeError, UnicodeError) as exc:
            raise BisqStreamError("Invalid private Bisq session JSON") from exc
        u = urlsplit(session.websocket_url)
        if (u.scheme != "ws" or u.hostname != "127.0.0.1" or u.port is None
                or u.username or u.password or u.query or u.fragment):
            raise BisqStreamError("Session URL must be an authenticated loopback WebSocket")
        return session

    ws = os.environ.get(f"RTG_{prefix}_WS_URL", "")
    cid = os.environ.get(f"RTG_{prefix}_CLIENT_ID", "")
    sid = os.environ.get(f"RTG_{prefix}_SESSION_ID", "")
    u = urlsplit(ws)
    if (
        u.scheme != "ws" or u.hostname != "127.0.0.1" or u.port is None
        or u.username or u.password or u.query or u.fragment
        or not cid or not sid
    ):
        raise BisqStreamError(f"Missing or unsafe local {prefix} session environment")
    return PairedSession(cid, "", sid, ws)


def main() -> None:
    parser = argparse.ArgumentParser(description="Read-only two-node Bisq / regtest correlator")
    parser.add_argument("--trade-id", required=True)
    parser.add_argument("--txid", required=True)
    parser.add_argument("--expected-sats", type=int, default=100_000)
    parser.add_argument("--rpc-url", default="http://127.0.0.1:18443")
    parser.add_argument("--listen-seconds", type=float, default=1)
    parser.add_argument("--session-dir", type=Path, help="Private session files from --export-sessions")
    args = parser.parse_args()
    try:
        alice = snapshot(
            _session("ALICE", args.session_dir), args.trade_id, args.expected_sats,
            listen_seconds=args.listen_seconds,
        )
        bob = snapshot(
            _session("BOB", args.session_dir), args.trade_id, args.expected_sats,
            listen_seconds=args.listen_seconds,
        )
        core = BitcoinCoreRPC(
            args.rpc_url,
            user=os.getenv("P2P_REGTEST_RPC_USER"),
            password=os.getenv("P2P_REGTEST_RPC_PASSWORD"),
        )
        print(json.dumps(
            verify_correlation(core, alice, bob, args.txid, args.expected_sats),
            sort_keys=True,
        ))
    except (BisqStreamError, RegtestError, OSError) as exc:
        parser.exit(1, f"Trade correlation refused: {exc}\n")


if __name__ == "__main__":
    main()
