"""Authenticated read-only Bisq 2 trade-state WebSocket observer.

Subscribe to TRADES and TRADE_PROPERTIES. Only the selected trade's
state/amount/validated regtest payment address enter the runtime model.
Private chat, raw payment data, keys and credentials are never logged.
"""

from __future__ import annotations

import json
import re
import time
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation
from typing import Any, Callable
from urllib.parse import parse_qsl, urlsplit

from .bisq_pairing import PairedSession
from .bitcoin_regtest import btc_to_sats

TRADE_RE = re.compile(r"^[a-zA-Z0-9_-]{6,128}$")
ADDRESS_RE = re.compile(r"^bcrt1[a-z0-9]{12,90}$")
STATE_RE = re.compile(r"^[A-Z_]{2,140}$")
FAILURE_STATES = {"REJECTED", "PEER_REJECTED", "CANCELLED", "PEER_CANCELLED", "FAILED", "FAILED_AT_PEER"}
PAYOUT_STATES = {"SELLER_SENT_BTC_SENT_CONFIRMATION", "BUYER_RECEIVED_BTC_SENT_CONFIRMATION", "BTC_CONFIRMED"}


class BisqStreamError(RuntimeError):
    pass


@dataclass
class TradeObservation:
    trade_id: str
    state: str | None = None
    base_sats: int | None = None
    regtest_address: str | None = None
    observed_payment_data: bool = False
    messages: int = 0

    def safe_report(self) -> dict[str, object]:
        return {
            "trade_id": self.trade_id,
            "state": self.state,
            "base_sats": self.base_sats,
            "regtest_payment_address_observed": self.regtest_address is not None,
            "messages": self.messages,
        }


def payment_address(data: str, expected_sats: int) -> str:
    """Only raw regtest bech32 or simple BIP21 on-chain payment data."""
    if not isinstance(data, str) or len(data) > 1024:
        raise BisqStreamError("Invalid payment data length")
    text = data.strip()
    if text.lower().startswith("bitcoin:"):
        uri = urlsplit(text)
        if uri.scheme != "bitcoin" or uri.netloc or uri.fragment:
            raise BisqStreamError("Invalid BIP21 payment URI")
        text = uri.path
        params = parse_qsl(uri.query, keep_blank_values=True, strict_parsing=True)
        keys = [key.lower() for key, _ in params]
        if len(set(keys)) != len(keys) or "lightning" in keys or any(key.startswith("req-") for key in keys):
            raise BisqStreamError("Unsafe or ambiguous BIP21 parameters")
        for key, value in params:
            if key.lower() == "amount":
                try:
                    if btc_to_sats(Decimal(value)) != expected_sats:
                        raise BisqStreamError("BIP21 request amount differs from contract")
                except (ValueError, InvalidOperation) as exc:
                    raise BisqStreamError("Invalid BIP21 amount") from exc
    if not ADDRESS_RE.fullmatch(text):
        raise BisqStreamError("Payment data is not an on-chain regtest bcrt1 address")
    return text


def _decode_json(raw: object) -> Any:
    if not isinstance(raw, str) or len(raw) > 1_000_000:
        raise BisqStreamError("Malformed or oversized WebSocket payload")
    try:
        return json.loads(raw)
    except (ValueError, UnicodeError) as exc:
        raise BisqStreamError("Malformed JSON WebSocket payload") from exc


def _update(
    views: dict[str, TradeObservation],
    topic: str,
    data: Any,
    trade_id: str,
    expected_sats: int,
    change: str = "REPLACE",
) -> None:
    if not isinstance(data, list) or len(data) > 1000:
        raise BisqStreamError("Invalid trade-state collection")
    if topic not in ("TRADES", "TRADE_PROPERTIES"):
        raise BisqStreamError("Unrecognized trade topic")
    for item in data:
        if not isinstance(item, dict):
            raise BisqStreamError("Malformed trade-state item")
        if topic == "TRADES":
            trade = item.get("trade")
            if not isinstance(trade, dict) or trade.get("id") != trade_id:
                continue
            if change == "REMOVED":
                views.pop(trade_id, None)
                continue
            props = trade
            amount = item.get("baseAmount")
            if amount is not None:
                if not isinstance(amount, int) or isinstance(amount, bool) or amount != expected_sats:
                    raise BisqStreamError("Bisq BTC contract amount mismatch")
        else:
            props = item.get(trade_id)
            if props is None:
                continue
            if not isinstance(props, dict):
                raise BisqStreamError("Bad trade property envelope")
        observed = views.setdefault(trade_id, TradeObservation(trade_id))
        if topic == "TRADES" and item.get("baseAmount") is not None:
            observed.base_sats = item["baseAmount"]
        state = props.get("tradeState")
        if state is not None:
            if not isinstance(state, str) or not STATE_RE.fullmatch(state):
                raise BisqStreamError("Malformed Bisq trade state")
            observed.state = state
        raw_payment = props.get("bitcoinPaymentData")
        if raw_payment is not None:
            observed.observed_payment_data = True
            observed.regtest_address = payment_address(raw_payment, expected_sats)
        observed.messages += 1


def consume_message(
    views: dict[str, TradeObservation],
    message: str,
    requests: dict[str, str],
    trade_id: str,
    expected_sats: int,
) -> str | None:
    message_data = _decode_json(message)
    if not isinstance(message_data, dict):
        raise BisqStreamError("Invalid subscription envelope")
    kind = message_data.get("type")
    if kind == "SubscriptionResponse":
        request_id = message_data.get("requestId")
        if request_id not in requests:
            raise BisqStreamError("Unexpected subscription request ID")
        if message_data.get("errorMessage"):
            raise BisqStreamError("Subscription refused by Bisq permissions")
        _update(
            views, requests[request_id], _decode_json(message_data.get("payload")),
            trade_id, expected_sats,
        )
        return request_id
    if kind == "WebSocketEvent":
        topic = message_data.get("topic")
        if topic not in ("TRADES", "TRADE_PROPERTIES"):
            return None
        change = message_data.get("modificationType")
        if change not in ("REPLACE", "ADDED", "REMOVED"):
            raise BisqStreamError("Invalid modificationType")
        _update(
            views, topic, _decode_json(message_data.get("payload")),
            trade_id, expected_sats, change=change,
        )
        return None
    raise BisqStreamError("Unsupported WebSocket envelope")


def snapshot(
    session: PairedSession,
    trade_id: str,
    expected_sats: int,
    *,
    timeout: float = 12.0,
    listen_seconds: float = 0.5,
    connect_fn: Callable[..., Any] | None = None,
) -> TradeObservation:
    if not TRADE_RE.fullmatch(trade_id) or not 1 <= expected_sats <= 100_000_000:
        raise BisqStreamError("Invalid trade ID or expected BTC amount")
    if not 1 <= timeout <= 60 or not 0 <= listen_seconds <= 30:
        raise BisqStreamError("Invalid capture time")
    ws_url = urlsplit(session.websocket_url)
    if ws_url.scheme != "ws" or ws_url.hostname != "127.0.0.1" or ws_url.username or ws_url.password:
        raise BisqStreamError("Only local authenticated WebSocket connections supported")
    if connect_fn is None:
        try:
            from websockets.sync.client import connect
        except ImportError as exc:
            raise BisqStreamError("Install optional websockets dependency") from exc
        connect_fn = connect
    requests = {"rtg-trades": "TRADES", "rtg-properties": "TRADE_PROPERTIES"}
    acked: set[str] = set()
    observations: dict[str, TradeObservation] = {}
    with connect_fn(
        session.websocket_url,
        additional_headers=session.headers(),
        open_timeout=timeout,
        close_timeout=2,
        max_size=1_000_000,
        compression=None,
    ) as conn:
        for rid, topic in requests.items():
            conn.send(json.dumps({"type": "SubscriptionRequest", "requestId": rid, "topic": topic}))
        deadline = time.monotonic() + timeout
        drain_until = None
        while time.monotonic() < deadline:
            if acked == set(requests):
                drain_until = drain_until or min(deadline, time.monotonic() + listen_seconds)
                if time.monotonic() >= drain_until:
                    break
            remaining = max(0.01, (drain_until or deadline) - time.monotonic())
            try:
                incoming = conn.recv(timeout=remaining)
            except TimeoutError:
                if acked == set(requests):
                    break
                raise BisqStreamError("Bisq WebSocket initial snapshot timed out") from None
            if not isinstance(incoming, str):
                raise BisqStreamError("Unexpected binary WebSocket frame")
            confirmed_request = consume_message(
                observations, incoming, requests, trade_id, expected_sats,
            )
            if confirmed_request:
                acked.add(confirmed_request)
        if acked != set(requests):
            raise BisqStreamError("Both trade subscriptions must acknowledge")
    return observations.get(trade_id, TradeObservation(trade_id))


def reconcile(
    alice: TradeObservation, bob: TradeObservation, expected_sats: int
) -> dict[str, object]:
    if alice.trade_id != bob.trade_id or not TRADE_RE.fullmatch(alice.trade_id):
        raise BisqStreamError("Alice and Bob trade IDs disagree")
    if not alice.state or not bob.state:
        raise BisqStreamError("Trade must be observed on both nodes")
    if alice.state in FAILURE_STATES or bob.state in FAILURE_STATES:
        raise BisqStreamError("One node reports a terminal failure state")
    for view in (alice, bob):
        if view.base_sats is not None and view.base_sats != expected_sats:
            raise BisqStreamError("Nodes disagree about contracted BTC amount")
    if alice.regtest_address and bob.regtest_address and alice.regtest_address != bob.regtest_address:
        raise BisqStreamError("Nodes disagree on BTC payment address")
    address = alice.regtest_address or bob.regtest_address
    if not address:
        raise BisqStreamError("No regtest payment address observed")
    both_sent = alice.state in PAYOUT_STATES and bob.state in PAYOUT_STATES
    return {
        "kind": "bisq_two_node_observation",
        "trade_id": alice.trade_id,
        "alice_state": alice.state,
        "bob_state": bob.state,
        "expected_sats": expected_sats,
        "both_advanced_to_btc": both_sent,
        "both_bisq_confirmed": alice.state == bob.state == "BTC_CONFIRMED",
        "recipient_address_observed": True,
    }
