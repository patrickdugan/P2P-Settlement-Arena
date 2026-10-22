"""Read-only, localhost-only Bisq 2 REST inspection.

This is a separate boundary from Bitcoin Core regtest. It never posts offer
takes, changes trade state, sends BTC, or sends fiat. Only explicit, verified
GET endpoints from the Bisq 2 REST source are permitted. Sensitive client
and session IDs remain in the HTTP request headers and are never printed.
"""

from __future__ import annotations

import argparse
import json
import os
import re
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPHandler, ProxyHandler, Request, build_opener


class BisqReadError(RuntimeError):
    """The local Bisq 2 API could not be inspected."""


class BisqReadOnlyClient:
    def __init__(
        self,
        base_url: str = "http://127.0.0.1:8090",
        *,
        client_id: str | None = None,
        session_id: str | None = None,
        timeout: float = 5.0,
    ) -> None:
        parts = urlsplit(base_url)
        if (
            parts.scheme != "http"
            or parts.hostname != "127.0.0.1"
            or parts.port is None
            or parts.username is not None
            or parts.password is not None
            or parts.path not in ("", "/")
            or parts.query
            or parts.fragment
        ):
            raise BisqReadError("Only loopback Bisq API endpoints are permitted")
        if bool(client_id) != bool(session_id):
            raise BisqReadError("Both Bisq-Client-Id and Bisq-Session-Id are required for pairing")
        if timeout <= 0 or timeout > 30:
            raise BisqReadError("Bisq API timeout must be in (0,30] seconds")
        self.base_url = f"http://127.0.0.1:{parts.port}/api/v1/"
        self.timeout = timeout
        self._headers = {"Accept": "application/json"}
        if client_id:
            self._headers["Bisq-Client-Id"] = client_id
            self._headers["Bisq-Session-Id"] = session_id or ""
        self._opener = build_opener(ProxyHandler({}), HTTPHandler())

    def _get(self, path: str) -> Any:
        request = Request(self.base_url + path, headers=self._headers, method="GET")
        try:
            with self._opener.open(request, timeout=self.timeout) as response:
                data = response.read(1_000_001)
        except HTTPError as error:
            raise BisqReadError(f"Bisq API returned HTTP {error.code}") from error
        except (URLError, OSError, TimeoutError) as error:
            raise BisqReadError("Local Bisq 2 REST API could not be reached") from error
        if len(data) > 1_000_000:
            raise BisqReadError("Bisq response exceeds configured size limit")
        try:
            return json.loads(data.decode("utf-8"))
        except (UnicodeError, json.JSONDecodeError) as error:
            raise BisqReadError("Bisq API did not return valid JSON") from error

    def market_quotes(self) -> Any:
        return self._get("market-price/quotes")

    def market_offers(self, currency: str = "USD") -> Any:
        if not re.fullmatch(r"[A-Z]{3,5}", currency):
            raise BisqReadError("Invalid market currency code")
        return self._get(f"offerbook/markets/{currency}/offers")

    def closed_trades(self, page_size: int = 10) -> Any:
        if not isinstance(page_size, int) or not 1 <= page_size <= 30:
            raise BisqReadError("Closed trade page size out of bounds")
        return self._get(f"trades/closed?page=1&pageSize={page_size}")

    @staticmethod
    def _count(value: Any) -> int | None:
        if isinstance(value, list):
            return len(value)
        if isinstance(value, dict):
            for key in ("items", "offers", "trades", "data", "quotes"):
                child = value.get(key)
                if isinstance(child, list):
                    return len(child)
                if key == "quotes" and isinstance(child, dict):
                    return len(child)
            return None
        return None

    def snapshot(self, currency: str = "USD", include_closed: bool = False) -> dict[str, Any]:
        quotes = self.market_quotes()
        offers = self.market_offers(currency)
        result = {
            "kind": "bisq_read_only_snapshot",
            "api": "bisq2-rest-v1",
            "market": currency,
            "quotes_available": isinstance(quotes, (list, dict)),
            "offer_count": self._count(offers),
            "trading_enabled": False,
            "read_only": True,
        }
        if include_closed:
            trades = self.closed_trades()
            result["closed_trade_page_count"] = self._count(trades)
        # Intentionally suppress account identifiers, chat messages, payment
        # data and other potentially sensitive contents in the CLI output.
        return result


def main() -> None:
    parser = argparse.ArgumentParser(description="Read-only local Bisq 2 market observer")
    parser.add_argument("--api-url", default="http://127.0.0.1:8090")
    parser.add_argument("--market", default="USD")
    parser.add_argument("--closed-trades", action="store_true")
    args = parser.parse_args()
    try:
        client = BisqReadOnlyClient(
            args.api_url,
            client_id=os.getenv("BISQ_CLIENT_ID"),
            session_id=os.getenv("BISQ_SESSION_ID"),
        )
        print(json.dumps(client.snapshot(args.market, args.closed_trades), sort_keys=True))
    except BisqReadError as error:
        parser.exit(1, f"Bisq observation failed: {error}\n")


if __name__ == "__main__":
    main()
