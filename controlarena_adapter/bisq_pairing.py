"""Bisq 2 pairing-code reader and explicitly authorized localhost session helper.

Protocol layout derived independently from Bisq's public PairingQrCodeEncoder,
PairingCodeEncoder and BinaryEncodingUtils. Pairing QR files, one-time codes,
client secrets and sessions are credentials: never log or upload their contents.
"""

from __future__ import annotations

import base64
import json
import re
import stat
import struct
import time
from dataclasses import dataclass, field
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPHandler, ProxyHandler, Request, build_opener

UUID_RE = re.compile(r"^[a-fA-F0-9]{8}(-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$")


class PairingError(RuntimeError):
    pass


class _Reader:
    def __init__(self, data: bytes) -> None:
        self.data = data
        self.offset = 0

    def take(self, count: int) -> bytes:
        if count < 0 or self.offset + count > len(self.data):
            raise PairingError("Truncated pairing code")
        start = self.offset
        self.offset += count
        return self.data[start:self.offset]

    def byte(self) -> int:
        return self.take(1)[0]

    def short_blob(self, maximum: int) -> bytes:
        n = int.from_bytes(self.take(2), "big")
        if n > maximum:
            raise PairingError("Pairing field exceeds format limit")
        return self.take(n)

    def text(self, maximum: int) -> str:
        try:
            return self.short_blob(maximum).decode("utf-8", errors="strict")
        except UnicodeError as exc:
            raise PairingError("Invalid pairing text encoding") from exc

    def int32(self) -> int:
        return struct.unpack(">i", self.take(4))[0]

    def int64(self) -> int:
        return struct.unpack(">q", self.take(8))[0]

    def complete(self) -> None:
        if self.offset != len(self.data):
            raise PairingError("Trailing data in pairing code")


@dataclass(frozen=True)
class PairingTicket:
    code_id: str = field(repr=False)
    websocket_url: str
    expires_ms: int

    def validate(self, expected_port: int) -> None:
        u = urlsplit(self.websocket_url)
        if (u.scheme != "ws" or u.hostname != "127.0.0.1"
                or u.port != expected_port or u.username or u.password
                or u.query or u.fragment):
            raise PairingError("Pairing QR points outside the expected local WebSocket")
        if self.expires_ms <= int(time.time() * 1000):
            raise PairingError("Bisq pairing code has expired")
        if not UUID_RE.fullmatch(self.code_id):
            raise PairingError("Invalid pairing code identifier")


@dataclass(frozen=True, repr=False)
class PairedSession:
    client_id: str
    client_secret: str = field(repr=False)
    session_id: str = field(repr=False)
    websocket_url: str

    def headers(self) -> dict[str, str]:
        return {
            "Bisq-Client-Id": self.client_id,
            "Bisq-Session-Id": self.session_id,
        }


def decode_ticket(token: str) -> PairingTicket:
    if len(token) > 6000:
        raise PairingError("QR token exceeds size cap")
    try:
        outer = _Reader(base64.urlsafe_b64decode(token + "=" * (-len(token) % 4)))
        if outer.byte() != 1:
            raise PairingError("Unsupported QR version")
        inner = _Reader(outer.short_blob(4096))
        if inner.byte() != 1:
            raise PairingError("Unsupported pairing protocol version")
        code_id = inner.text(128)
        expires_ms = inner.int64()
        permissions = inner.int32()
        if not 0 <= permissions <= 128:
            raise PairingError("Invalid permission count")
        for _ in range(permissions):
            inner.int32()
        inner.complete()
        websocket_url = outer.text(512)
        flags = outer.byte()
        if flags & ~0b11:
            raise PairingError("Unexpected pairing QR flags")
        if flags & 1:
            # local CLEAR profiles should not use a TLS fingerprint
            outer.short_blob(128)
            raise PairingError("TLS pairing QR requires a different, verified transport")
        if flags & 2:
            outer.short_blob(256)
            raise PairingError("Tor pairing QR is excluded from local clearnet harness")
        outer.complete()
        return PairingTicket(code_id=code_id, websocket_url=websocket_url, expires_ms=expires_ms)
    except (ValueError, struct.error) as exc:
        raise PairingError("Invalid pairing QR encoding") from exc


def read_pairing_file(path: Path, expected_port: int) -> PairingTicket:
    path = Path(path)
    meta = path.lstat()
    if not stat.S_ISREG(meta.st_mode) or meta.st_size > 65536:
        raise PairingError("Pairing file must be a small, regular local file")
    if meta.st_mode & 0o077:
        raise PairingError("Pairing QR file must have private 0600 permissions")
    # Upstream appends a Unicode block-character QR rendering after line one.
    # Read the secret Base64 header as raw bytes, avoiding decoding the artwork.
    with path.open("rb") as stream:
        encoded = stream.readline(6001).strip()
    if not encoded or len(encoded) > 6000:
        raise PairingError("Pairing QR token is empty or too long")
    try:
        token = encoded.decode("ascii")
    except UnicodeError as exc:
        raise PairingError("Pairing QR token contains non-ASCII bytes") from exc
    ticket = decode_ticket(token)
    ticket.validate(expected_port)
    return ticket


def _post_local(port: int, endpoint: str, request_body: dict[str, object]) -> dict:
    if not 1 <= port <= 65535 or endpoint not in ("pairing", "session"):
        raise PairingError("Invalid local pairing target")
    url = f"http://127.0.0.1:{port}/api/v1/access/{endpoint}"
    req = Request(
        url,
        method="POST",
        data=json.dumps(request_body).encode(),
        headers={"Accept": "application/json", "Content-Type": "application/json"},
    )
    opener = build_opener(ProxyHandler({}), HTTPHandler())
    try:
        with opener.open(req, timeout=5) as response:
            raw = response.read(8193)
    except HTTPError as exc:
        raise PairingError(f"Local Bisq {endpoint} returned HTTP {exc.code}") from exc
    except (URLError, OSError) as exc:
        raise PairingError("Local Bisq access service unavailable") from exc
    if len(raw) > 8192:
        raise PairingError("Oversized Bisq pairing response")
    try:
        data = json.loads(raw)
    except (ValueError, UnicodeError) as exc:
        raise PairingError("Malformed Bisq access response") from exc
    if not isinstance(data, dict):
        raise PairingError("Bisq access response is not an object")
    return data


def pair_node(ticket: PairingTicket, port: int, client_name: str) -> PairedSession:
    ticket.validate(port)
    if not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", client_name):
        raise PairingError("Unsafe or excessive client name")
    data = _post_local(port, "pairing", {
        "version": 1,
        "pairingCodeId": ticket.code_id,
        "clientName": client_name,
    })
    values = [data.get(key) for key in ("clientId", "clientSecret", "sessionId")]
    if any(not isinstance(v, str) or len(v) < 8 for v in values):
        raise PairingError("Bisq pairing response missing credentials")
    return PairedSession(values[0], values[1], values[2], ticket.websocket_url)
