"""External Bisq two-node integration boundaries with fake endpoints only.

Tests neither open real trade accounts nor need a running Bisq, Bitcoin Core,
Java, GitHub, or websocket server. Live launch remains explicitly opt-in.
"""

import base64
import json
import os
import struct
import tempfile
import time
import unittest
from decimal import Decimal
from pathlib import Path
from unittest.mock import patch

from controlarena_adapter.bisq_pairing import (
    PairingError, PairedSession, decode_ticket, pair_node, read_pairing_file,
)
from controlarena_adapter.bisq_two_node import BisqPair, NodeError, plan
from controlarena_adapter.bisq_ws import (
    BisqStreamError, TradeObservation, consume_message, payment_address,
    reconcile, snapshot,
)
from controlarena_adapter.bisq_correlate import verify_correlation

TRADE_ID = "trade_abcdefgh0123456"
REGTEST_ADDR = "bcrt1q" + "p" * 36
TICKET_ID = "123e4567-e89b-12d3-a456-426614174000"


def _blob(data):
    return struct.pack(">H", len(data)) + data


def _qr_token(port, expires_ms=None, code_id=TICKET_ID):
    if expires_ms is None:
        expires_ms = int(time.time() * 1000) + 120_000
    code = (b"\x01" + _blob(code_id.encode()) + struct.pack(">q", expires_ms)
            + struct.pack(">i", 1) + struct.pack(">i", 9))
    raw = (b"\x01" + _blob(code)
           + _blob(f"ws://127.0.0.1:{port}".encode())
           + b"\x00")
    return base64.urlsafe_b64encode(raw).decode().rstrip("=")


def _item(state, payment=None):
    return {
        "trade": {
            "id": TRADE_ID,
            "tradeState": state,
            "bitcoinPaymentData": payment,
        },
        "baseAmount": 100_000,
    }


def _frame(rid, data):
    return json.dumps({
        "type": "SubscriptionResponse", "requestId": rid,
        "payload": json.dumps(data), "errorMessage": None,
    })


class PairingTests(unittest.TestCase):
    def test_decodes_upstream_big_endian_length_prefixed_ticket(self):
        ticket = decode_ticket(_qr_token(18600))
        ticket.validate(18600)
        self.assertEqual(ticket.code_id, TICKET_ID)
        with self.assertRaises(PairingError):
            ticket.validate(18601)

    def test_rejects_expiry_invalid_flags_and_truncation(self):
        expired = decode_ticket(_qr_token(18600, expires_ms=0))
        with self.assertRaises(PairingError):
            expired.validate(18600)
        valid = _qr_token(18600)
        with self.assertRaises(PairingError):
            decode_ticket(valid[:-8])
        with self.assertRaises(PairingError):
            decode_ticket("!notbase64!")

    def test_private_pairing_file_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pairing_qr_code.txt"
            path.write_text(_qr_token(18600) + "\n\n██▄▀ █▀▄ ▀▄██\n")
            path.chmod(0o600)
            self.assertEqual(read_pairing_file(path, 18600).code_id, TICKET_ID)
            path.chmod(0o644)
            with self.assertRaisesRegex(PairingError, "0600"):
                read_pairing_file(path, 18600)

    def test_pairing_token_first_line_must_be_ascii(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pairing_qr_code.txt"
            path.write_bytes(bytes((0xff, 0xfe, 0x0a)) + _qr_token(18600).encode("ascii"))
            path.chmod(0o600)
            with self.assertRaisesRegex(PairingError, "non-ASCII"):
                read_pairing_file(path, 18600)

    @patch("controlarena_adapter.bisq_pairing._post_local")
    def test_pairing_secret_stays_in_memory(self, post):
        post.return_value = {
            "clientId": "client-1234567890",
            "clientSecret": "hiddensecret123",
            "sessionId": "sessionabc12345",
        }
        result = pair_node(decode_ticket(_qr_token(18600)), 18600, "rtg_alice")
        self.assertEqual(result.headers()["Bisq-Session-Id"], "sessionabc12345")
        self.assertEqual(result.websocket_url, "ws://127.0.0.1:18600/websocket")
        self.assertNotIn("hiddensecret123", repr(result))
        args = post.call_args.args
        self.assertEqual(args[1], "pairing")
        self.assertEqual(args[2]["version"], 1)
        self.assertEqual(args[2]["pairingCodeId"], TICKET_ID)


class Subscriptions(unittest.TestCase):
    def test_payment_data_rejects_overpay_and_wrong_network(self):
        self.assertEqual(
            payment_address(f"bitcoin:{REGTEST_ADDR}?amount=0.00100000", 100_000),
            REGTEST_ADDR,
        )
        for data in (
            f"bitcoin:{REGTEST_ADDR}?amount=0.01",
            f"bitcoin:{REGTEST_ADDR}?amount=0.001&amount=0.01",
            f"bitcoin:{REGTEST_ADDR}?lightning=lnbc1bad",
            "bc1qmainnetvalue12345678901234567890",
            "lnbc123456",
        ):
            with self.subTest(data=data):
                with self.assertRaises(BisqStreamError):
                    payment_address(data, 100_000)

    def test_subscription_envelopes_and_state_patch(self):
        views = {}
        requests = {"rtg-trades": "TRADES", "rtg-properties": "TRADE_PROPERTIES"}
        rid = consume_message(
            views,
            _frame("rtg-trades", [_item("SELLER_SENT_BTC_SENT_CONFIRMATION")]),
            requests, TRADE_ID, 100_000,
        )
        self.assertEqual(rid, "rtg-trades")
        consume_message(
            views,
            _frame("rtg-properties", [{TRADE_ID: {
                "tradeState": "BTC_CONFIRMED",
                "bitcoinPaymentData": f"bitcoin:{REGTEST_ADDR}?amount=0.001",
            }}]),
            requests, TRADE_ID, 100_000,
        )
        self.assertEqual(views[TRADE_ID].state, "BTC_CONFIRMED")
        self.assertEqual(views[TRADE_ID].base_sats, 100_000)
        self.assertEqual(views[TRADE_ID].regtest_address, REGTEST_ADDR)
        self.assertNotIn(REGTEST_ADDR, json.dumps(views[TRADE_ID].safe_report()))

    def test_permission_error_refused(self):
        with self.assertRaisesRegex(BisqStreamError, "refused"):
            consume_message(
                {}, json.dumps({
                    "type": "SubscriptionResponse", "requestId": "rtg-trades",
                    "errorMessage": "permission_not_granted: TRADES",
                    "payload": None,
                }),
                {"rtg-trades": "TRADES"}, TRADE_ID, 100_000,
            )

    def test_sync_websocket_is_read_only_and_bounded(self):
        class FakeWs:
            def __init__(self):
                self.sent = []
                self.incoming = [
                    _frame("rtg-trades", [_item("BTC_CONFIRMED", REGTEST_ADDR)]),
                    _frame("rtg-properties", [{TRADE_ID: {"tradeState": "BTC_CONFIRMED"}}]),
                ]
            def __enter__(self):
                return self
            def __exit__(self, *args):
                return None
            def send(self, raw):
                self.sent.append(json.loads(raw))
            def recv(self, timeout=None):
                if self.incoming:
                    return self.incoming.pop(0)
                raise TimeoutError()

        mock = FakeWs()
        session = PairedSession("cid", "", "sid", "ws://127.0.0.1:18600/websocket")
        view = snapshot(session, TRADE_ID, 100_000, connect_fn=lambda *args, **kw: mock)
        self.assertEqual(view.state, "BTC_CONFIRMED")
        self.assertEqual([v["topic"] for v in mock.sent], ["TRADES", "TRADE_PROPERTIES"])
        self.assertTrue(all(v["type"] == "SubscriptionRequest" for v in mock.sent))
        self.assertEqual(reconcile(view, view, 100_000)["both_bisq_confirmed"], True)


class CorrelatorTests(unittest.TestCase):
    class FakeCore:
        def __init__(self, confirmations=2, credited_sats=100_000):
            self.checked = False
            self.confirmations = confirmations
            self.credited_sats = credited_sats
        def assert_regtest(self):
            self.checked = True
        def guarded(self, method, args):
            assert self.checked and method == "getrawtransaction"
            return {
                "txid": "a" * 64,
                "confirmations": self.confirmations,
                "vout": [{
                    "value": Decimal(self.credited_sats) / Decimal(100_000_000),
                    "scriptPubKey": {"address": REGTEST_ADDR},
                }],
            }

    @staticmethod
    def _view():
        return TradeObservation(
            TRADE_ID, "BTC_CONFIRMED", 100_000, REGTEST_ADDR, True, 3,
        )

    def test_requires_two_advanced_states_and_actual_chain_payout(self):
        result = verify_correlation(
            self.FakeCore(), self._view(), self._view(), "a" * 64, 100_000,
        )
        self.assertTrue(result["passed"])
        self.assertEqual(result["confirmed_sats"], 100_000)
        self.assertTrue(result["both_bisq_confirmed"])
        self.assertFalse(result["fiat_finality_verified"])

    def test_rejects_missing_node_state_wrong_amount_and_unconfirmed(self):
        with self.assertRaises(BisqStreamError):
            verify_correlation(
                self.FakeCore(), self._view(),
                TradeObservation(TRADE_ID), "a" * 64, 100_000,
            )
        with self.assertRaises(BisqStreamError):
            verify_correlation(
                self.FakeCore(credited_sats=99_999),
                self._view(), self._view(), "a" * 64, 100_000,
            )
        with self.assertRaises(BisqStreamError):
            verify_correlation(
                self.FakeCore(confirmations=0),
                self._view(), self._view(), "a" * 64, 100_000,
            )
        wrong = TradeObservation("differenttrade_abcdef", "BTC_CONFIRMED", 100_000, REGTEST_ADDR)
        with self.assertRaises(BisqStreamError):
            verify_correlation(self.FakeCore(), self._view(), wrong, "a" * 64, 100_000)


class LauncherPlanTests(unittest.TestCase):
    def test_seed_and_nodes_have_distinct_dirs_and_ports(self):
        with tempfile.TemporaryDirectory() as tmp:
            nodes = plan(Path(tmp) / "bisq2", Path(tmp) / "state")
            self.assertEqual([n.p2p_port for n in nodes], [18500, 18501, 18502])
            self.assertEqual([n.api_port for n in nodes], [None, 18600, 18601])
            self.assertEqual(len({n.data_dir for n in nodes}), 3)
            self.assertIn("supportedTransportTypes.0=CLEAR", nodes[2].java_opts())
            self.assertIn("bind.host=127.0.0.1", nodes[1].java_opts())
            self.assertNotIn("authorizationRequired=false", nodes[1].java_opts())
        for value in (0, 1023, 64001):
            with self.assertRaises(NodeError):
                plan(Path("/tmp/a"), Path("/tmp/b"), value)

    def test_launcher_refuses_missing_binaries_and_does_not_spawn(self):
        with tempfile.TemporaryDirectory() as tmp:
            nodes = plan(Path(tmp) / "missing", Path(tmp) / "newdata")
            with patch("controlarena_adapter.bisq_two_node.subprocess.Popen") as spawn:
                with self.assertRaises(NodeError):
                    with BisqPair(nodes):
                        pass
                spawn.assert_not_called()


if __name__ == "__main__":
    unittest.main()
