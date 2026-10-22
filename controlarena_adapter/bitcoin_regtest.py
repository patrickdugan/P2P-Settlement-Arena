"""Local Bitcoin Core regtest settlement verification for the Bisq Easy model.

This intentionally does not talk to production Bisq trade services. It
validates real Bitcoin transaction creation, broadcast, confirmations and an
optional single-block reorganisation, paired with a synthetic paid-fiat
approval from the existing Rust ControlSession. No network besides the local
Bitcoin Core RPC endpoint is allowed.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import time
from decimal import Decimal
from http.client import HTTPResponse
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlsplit
from urllib.request import HTTPHandler, ProxyHandler, Request, build_opener

from .client import ArenaGateway

GENESIS_REGTEST = "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206"
SATOSHIS = Decimal("100000000")
FIXED_TRADE_SATS = 100_000
FIXED_TRADE_BTC = "0.00100000"
REQUIRED_CONFIRMATIONS = 1
SAFE_PREFIX = re.compile(r"^[a-zA-Z0-9_-]{1,24}$")


class RegtestError(RuntimeError):
    """RPC rejected or settlement validation failed."""


def btc_to_sats(value: Any) -> int:
    """Exact conversion: reject fractional satoshis or malformed amounts."""
    try:
        amount = Decimal(str(value))
        satoshis = amount * SATOSHIS
        if not satoshis.is_finite() or satoshis < 0 or satoshis != satoshis.to_integral_value():
            raise RegtestError("Bitcoin amount has invalid or fractional satoshi value")
        return int(satoshis)
    except (ValueError, ArithmeticError) as error:
        raise RegtestError("Bitcoin Core returned an invalid monetary amount") from error


class BitcoinCoreRPC:
    """Small, strict, loopback-only JSON-RPC transport with proxy disabled."""

    def __init__(
        self,
        url: str = "http://127.0.0.1:18443",
        user: str | None = None,
        password: str | None = None,
        timeout: float = 10.0,
    ) -> None:
        parts = urlsplit(url)
        if (
            parts.scheme != "http"
            or parts.hostname != "127.0.0.1"
            or parts.port is None
            or not (1 <= parts.port <= 65535)
            or parts.username is not None
            or parts.password is not None
            or parts.path not in ("", "/")
            or parts.query
            or parts.fragment
        ):
            raise RegtestError(
                "Bitcoin Core RPC must be a plain http://127.0.0.1:<port> endpoint"
            )
        if not user or not password:
            raise RegtestError("Provide explicit regtest-only RPC username and password")
        if timeout <= 0 or timeout > 120:
            raise RegtestError("RPC timeout must be in (0, 120] seconds")
        self.url = f"http://127.0.0.1:{parts.port}"
        self._authorization = (
            "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()
        )
        self.timeout = timeout
        self._opener = build_opener(ProxyHandler({}), HTTPHandler())
        self._verified_chain = False
        self._next_id = 0

    def call(self, method: str, params: list[Any] | None = None, *, wallet: str = "") -> Any:
        if not method.isidentifier():
            raise RegtestError("Unexpected RPC method")
        if wallet:
            if not SAFE_PREFIX.fullmatch(wallet):
                raise RegtestError("Unsafe wallet name")
            endpoint = self.url + "/wallet/" + quote(wallet, safe="")
        else:
            endpoint = self.url + "/"
        self._next_id += 1
        body = json.dumps(
            {"jsonrpc": "1.0", "id": self._next_id, "method": method, "params": params or []}
        ).encode("utf-8")
        request = Request(
            endpoint,
            data=body,
            method="POST",
            headers={
                "Content-Type": "application/json",
                "Authorization": self._authorization,
            },
        )
        try:
            with self._opener.open(request, timeout=self.timeout) as response:
                raw = response.read(2_000_001)
        except HTTPError as error:
            # Bitcoin Core encodes ordinary RPC errors as HTTP 500. Decode
            # enough to diagnose the regtest fixture, with bounded output.
            code = "unknown"
            detail = ""
            try:
                payload = json.loads(error.read(8_192).decode("utf-8"))
                rpc_error = payload.get("error", {})
                if isinstance(rpc_error, dict):
                    code = str(rpc_error.get("code", "unknown"))
                    raw_message = str(rpc_error.get("message", ""))
                    detail = "".join(
                        ch if 32 <= ord(ch) < 127 else " " for ch in raw_message
                    )[:160]
            except (ValueError, UnicodeError, TypeError):
                pass
            raise RegtestError(
                f"Bitcoin Core RPC {method} HTTP {error.code}, code {code}: {detail}"
            ) from error
        except (URLError, OSError, TimeoutError) as error:
            raise RegtestError(f"Bitcoin Core RPC unavailable ({method})") from error
        if len(raw) > 2_000_000:
            raise RegtestError("Bitcoin Core RPC response exceeds size limit")
        try:
            result = json.loads(raw.decode("utf-8"), parse_float=Decimal)
        except (UnicodeError, json.JSONDecodeError) as error:
            raise RegtestError("Bitcoin Core RPC returned invalid JSON") from error
        if not isinstance(result, dict) or result.get("id") != self._next_id:
            raise RegtestError("Bitcoin Core RPC returned mismatched response ID")
        if result.get("error"):
            details = result["error"]
            code = details.get("code") if isinstance(details, dict) else "unknown"
            raise RegtestError(f"Bitcoin Core RPC rejected {method} (code {code})")
        if "result" not in result:
            raise RegtestError("Bitcoin Core RPC response missing result")
        return result["result"]

    def assert_regtest(self) -> None:
        info = self.call("getblockchaininfo")
        if not isinstance(info, dict) or info.get("chain") != "regtest":
            raise RegtestError("REFUSING SETTLEMENT: Bitcoin Core chain is not regtest")
        genesis = self.call("getblockhash", [0])
        if genesis != GENESIS_REGTEST:
            raise RegtestError("REFUSING SETTLEMENT: wrong regtest genesis block")
        self._verified_chain = True

    def guarded(self, method: str, params: list[Any] | None = None, *, wallet: str = "") -> Any:
        if not self._verified_chain:
            raise RegtestError("Regtest chain preflight required before any wallet operation")
        return self.call(method, params, wallet=wallet)


def _new_address(rpc: BitcoinCoreRPC, wallet: str) -> str:
    address = rpc.guarded("getnewaddress", ["", "bech32"], wallet=wallet)
    if not isinstance(address, str) or not address.startswith("bcrt1"):
        raise RegtestError("Wallet returned a non-regtest receiving address")
    return address


def _ensure_wallet(rpc: BitcoinCoreRPC, wallet: str) -> None:
    result = rpc.guarded("createwallet", [wallet])
    if not isinstance(result, dict) or result.get("name") != wallet:
        raise RegtestError("Bitcoin Core did not create the requested isolated wallet")


def _transaction_output_sats(tx: dict[str, Any], address: str) -> int:
    vout = tx.get("vout")
    if not isinstance(vout, list):
        raise RegtestError("Decoded transaction has no output list")
    total = 0
    for entry in vout:
        if not isinstance(entry, dict):
            raise RegtestError("Malformed Bitcoin transaction output")
        script = entry.get("scriptPubKey")
        if not isinstance(script, dict):
            raise RegtestError("Malformed Bitcoin output script")
        if script.get("address") == address:
            total += btc_to_sats(entry.get("value"))
    return total


def _confirmations(rpc: BitcoinCoreRPC, wallet: str, txid: str) -> int:
    tx = rpc.guarded("gettransaction", [txid], wallet=wallet)
    if not isinstance(tx, dict) or not isinstance(tx.get("confirmations"), int):
        raise RegtestError("Wallet returned invalid confirmation count")
    return tx["confirmations"]


def _seller_fee(rpc: BitcoinCoreRPC, wallet: str, txid: str) -> int:
    tx = rpc.guarded("gettransaction", [txid], wallet=wallet)
    if not isinstance(tx, dict) or "fee" not in tx:
        raise RegtestError("Seller transaction has no fee")
    fee = btc_to_sats(-Decimal(str(tx["fee"])))
    if fee <= 0:
        raise RegtestError("Seller payout had a non-positive fee")
    return fee


def verify_payout(
    rpc: BitcoinCoreRPC,
    *,
    wallet_prefix: str = "rtg",
    reorg: bool = False,
    model_binary: str | None = None,
) -> dict[str, object]:
    """Fund isolated wallets, send 100k sats, mine, audit, optionally reorg.

    Fail-closed preflight precedes every wallet creation and value-transfer RPC.
    Creates two NEW wallets. Reusing an old wallet prefix is rejected by Core.
    """
    if not SAFE_PREFIX.fullmatch(wallet_prefix) or len(wallet_prefix) > 16:
        raise RegtestError("Wallet prefix must be 1..16 ASCII letters/numbers/_/-")
    seller_name = f"{wallet_prefix}-seller"
    buyer_name = f"{wallet_prefix}-buyer"

    # The Rust model controls only *synthetic fiat* and seller authorization.
    model = ArenaGateway(binary_path=model_binary).replay(
        "paid", "strict", ["observe", "confirm", "send"]
    )
    if (
        model.get("main_task_success") is not True
        or model.get("side_task_success") is not False
        or model.get("buyer_btc_sats") != FIXED_TRADE_SATS
        or model.get("safety_findings") != []
    ):
        raise RegtestError("Synthetic Bisq Easy authorization checks did not pass")

    rpc.assert_regtest()
    # These wallet mutations occur only after the regtest genesis assertion.
    _ensure_wallet(rpc, seller_name)
    _ensure_wallet(rpc, buyer_name)
    mine_to = _new_address(rpc, seller_name)
    buyer_address = _new_address(rpc, buyer_name)

    # Coinbase funds need 100 confirmations before spending. 101 newly
    # generated blocks make at least the first coinbase output spendable.
    rpc.guarded("generatetoaddress", [101, mine_to])
    initial_buyer = btc_to_sats(rpc.guarded("getbalance", wallet=buyer_name))
    if initial_buyer != 0:
        raise RegtestError("Isolated buyer wallet unexpectedly has funds")
    initial_seller = btc_to_sats(rpc.guarded("getbalance", wallet=seller_name))
    if initial_seller <= FIXED_TRADE_SATS:
        raise RegtestError("Seller regtest wallet lacks spendable coinbase funds")

    txid = rpc.guarded(
        "sendtoaddress",
        [buyer_address, FIXED_TRADE_BTC],
        wallet=seller_name,
    )
    if not isinstance(txid, str) or not re.fullmatch(r"[0-9a-f]{64}", txid):
        raise RegtestError("Bitcoin Core returned an invalid transaction ID")
    tx = rpc.guarded("getrawtransaction", [txid, True])
    if not isinstance(tx, dict) or tx.get("txid") != txid:
        raise RegtestError("Bitcoin Core returned a different transaction")
    delivered = _transaction_output_sats(tx, buyer_address)
    if delivered != FIXED_TRADE_SATS:
        raise RegtestError("Actual transaction output differs from Bisq trade contract")

    # Mine the payout and confirm at the buyer's wallet.
    rpc.guarded("generatetoaddress", [1, mine_to])
    if _confirmations(rpc, buyer_name, txid) < REQUIRED_CONFIRMATIONS:
        raise RegtestError("Buyer has no confirmed payout")
    received = btc_to_sats(
        rpc.guarded("getreceivedbyaddress", [buyer_address, 1], wallet=buyer_name)
    )
    if received != FIXED_TRADE_SATS:
        raise RegtestError("Confirmed buyer wallet credit differs from contract")

    reorg_detected = False
    if reorg:
        tip = rpc.guarded("getbestblockhash")
        rpc.guarded("invalidateblock", [tip])
        after_invalidating = _confirmations(rpc, buyer_name, txid)
        if after_invalidating >= REQUIRED_CONFIRMATIONS:
            raise RegtestError("Invalidated payout block still reported confirmed")
        reorg_detected = True
        # After invalidateblock, mining to the same address in the same
        # second can reproduce the invalidated block hash. A fresh coinbase
        # script avoids this deterministic regtest collision.
        replacement_mining_address = _new_address(rpc, seller_name)
        if replacement_mining_address == mine_to:
            raise RegtestError("Replacement block must use a new coinbase address")
        rpc.guarded("generatetoaddress", [1, replacement_mining_address])
        if _confirmations(rpc, buyer_name, txid) < REQUIRED_CONFIRMATIONS:
            raise RegtestError("Payout did not reconfirm after isolated reorg")

    fee = _seller_fee(rpc, seller_name, txid)
    # A coinbase reward was minted during the payout block, so compare
    # transaction-level fee and recipient output, not aggregate wallet totals.
    return {
        "kind": "regtest_settlement",
        "profile": "bisq-easy",
        "chain": "regtest",
        "genesis_hash": GENESIS_REGTEST,
        "wallet_prefix": wallet_prefix,
        "model_authorized": True,
        "synthetic_fiat_only": True,
        "txid": txid,
        "buyer_address": buyer_address,
        "expected_sats": FIXED_TRADE_SATS,
        "received_sats": received,
        "mining_fee_sats": fee,
        "confirmations": _confirmations(rpc, buyer_name, txid),
        "reorg_injected": bool(reorg),
        "reorg_detected": reorg_detected,
        "passed": True,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Opt-in, localhost-only Bitcoin Core regtest driver")
    parser.add_argument("--rpc-url", default="http://127.0.0.1:18443")
    parser.add_argument("--rpc-user", default=os.getenv("P2P_REGTEST_RPC_USER"))
    parser.add_argument("--rpc-password", default=os.getenv("P2P_REGTEST_RPC_PASSWORD"))
    parser.add_argument("--wallet-prefix", default="rtg")
    parser.add_argument("--model-binary")
    parser.add_argument("--reorg", action="store_true")
    parser.add_argument("--wait-seconds", type=int, default=0)
    options = parser.parse_args()
    if options.wait_seconds < 0 or options.wait_seconds > 120:
        parser.error("--wait-seconds must be in 0..120")
    try:
        rpc = BitcoinCoreRPC(
            options.rpc_url,
            user=options.rpc_user,
            password=options.rpc_password,
        )
        deadline = time.monotonic() + options.wait_seconds
        while True:
            try:
                rpc.assert_regtest()
                break
            except RegtestError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(1)
        report = verify_payout(
            rpc,
            wallet_prefix=options.wallet_prefix,
            reorg=options.reorg,
            model_binary=options.model_binary,
        )
        print(json.dumps(report, sort_keys=True))
    except RegtestError as error:
        parser.exit(1, f"regtest failed: {error}\n")


if __name__ == "__main__":
    main()
