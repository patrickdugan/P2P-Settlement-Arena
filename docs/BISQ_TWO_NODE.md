# Two-node Bisq 2 harness and regtest trade correlation

An experimental companion to P2P Settlement Arena. It launches three
separate upstream Bisq 2 processes: one local CLEAR seed and two Bisq
API-app nodes representing Alice and Bob. Each node gets an independent
private data directory, network port, and paired API client. Only the
Bitcoin Core regtest adapter handles blockchain RPC.

The first working boundary is a *real Bisq trade-state observer* built on
the upstream TRADES and TRADE_PROPERTIES WebSocket topics. It can correlate
a manually supplied, confirmed Bitcoin Core regtest TXID with both nodes'
reported Bisq trade ID, BTC amount and regtest on-chain payment address.

## Scope

- Upstream Bisq API apps are genuine Java services; user identities, offers
  and trades have to exist for trade observations to contain data.
- The launcher bootstraps and pairs Alice and Bob. It does **not** create
  user identities, publish offers, accept offers, or claim fiat transfers.
- Real Bisq Easy is fiat-first and does not internally custodially escrow BTC.
- A regtest correlation proves that an existing transaction pays the address
  observed in both independent local API nodes for the expected amount.
  It does not prove fiat finality, that Bisq software authored/signed the
  transaction, or that any party on Bitcoin mainnet paid real money.
- Trade-related APIs remain subscription-only and read-only here.
  The optional existing Bitcoin Core driver sends BTC exclusively on regtest.
- The two Bisq data profiles are isolated locally; the app is configured
  CLEAR-only for P2P bootstrap. The Java app may still have independently
  configured external market price/reference-time services. Use an OS/container
  egress firewall to guarantee network isolation.

## Build upstream Bisq (Java 21)

In a separate checkout:

~~~bash
git clone https://github.com/bisq-network/bisq2.git /tmp/rtg-bisq2
cd /tmp/rtg-bisq2
./gradlew :apps:seed-node-app:installDist :apps:api-app:installDist
~~~

This code imports no AGPL source into the arena repository. Upstream Bisq 2
is AGPL-3.0. Current upstream launcher conventions were checked against
scripts/local-3node.sh and apps/api-app/src/main/resources/api_app.conf.

## Plan and launch

From this P2P Settlement Arena checkout:

~~~bash
python -m pip install -e '.[bisq2]'
python -m controlarena_adapter.bisq_two_node \
  --bisq-root /tmp/rtg-bisq2 --plan

python -m controlarena_adapter.bisq_two_node \
  --bisq-root /tmp/rtg-bisq2 --launch --hold 600 --export-sessions
~~~

The API nodes normally use:
- seed local P2P 18500
- Alice local P2P 18501, REST/WebSocket 18600
- Bob local P2P 18502, REST/WebSocket 18601

Choose a distinct --port-base to avoid clashes. The --launch flag is
required for subprocess execution. The process owner controls all apps.
The launcher checks executable paths, creates fresh 0700 data directories,
starts the seed before Alice/Bob, waits for API readiness, and reads each
locally written private pairing QR code. It POSTs version 1 pairing to
/api/v1/access/pairing, retaining client credentials only in memory.

--export-sessions is an explicit opt-in to write private 0600 JSON session
files under the reported temporary_workspace/sessions directory. These
include client secret and session token, which are credentials; never
commit, share, or print them. The temporary workspace is removed when the
launcher exits. Logs under node data directories may contain private
information and are not uploaded to CI.

The upstream QR format uses an outer version byte, unsigned-short length
prefix, embedded pairing code, WebSocket URL, and transport flags. This
parser allows only local ws://127.0.0.1 URLs and rejects expired pairing
codes and unsupported TLS/Tor paths. User-facing QR text is never echoed.

## Read-only trade observation

With an existing Bisq Easy trade ID available on both paired nodes:

~~~bash
python -m controlarena_adapter.bisq_correlate \
  --session-dir /tmp/YOUR-PRINTED-WORKSPACE/sessions \
  --trade-id YOUR_MATCHING_TRADE_ID \
  --txid YOUR_CONFIRMED_REGTEST_TXID \
  --expected-sats 100000
~~~

Set P2P_REGTEST_RPC_USER and P2P_REGTEST_RPC_PASSWORD for the localhost
Bitcoin Core regtest node as documented in REGTEST.md. A client may also
provide RTG_ALICE_CLIENT_ID, RTG_ALICE_SESSION_ID, RTG_ALICE_WS_URL (and the
corresponding RTG_BOB variables) instead of --session-dir.

The observer sends only two authorized WebSocket subscriptions per node:
TRADES and TRADE_PROPERTIES. It verifies matching requested trade IDs,
checks base BTC amount against expected sats, validates payment data as an
on-chain BIP21 or raw bcrt1 regtest address, and rejects unsupported
Lightning/overpaid payment requests. If both authenticated nodes observe
the payout stage and the same regtest recipient address, the read-only
Bitcoin Core client checks the supplied TXID's decoded output and
confirmations. The report includes per-node states and a chain confirmation
count. It suppresses raw payment request strings and paired credentials.

One may instead ask the launcher itself to observe an existing trade:

~~~bash
python -m controlarena_adapter.bisq_two_node \
  --bisq-root /tmp/rtg-bisq2 --launch --trade-id YOUR_TRADE_ID \
  --trade-wait 120 --hold 0
~~~

This expects both apps to receive the trade while the launcher runs. With
--txid YOUR_REGTEST_TXID, it also verifies the chain correlation. As the
launcher creates *fresh* profiles, a trade must be generated using the
upstream apps or a separately authorized client. The tool itself is
strictly read-only for trade actions.

## Protocol observations and regression tests

The upstream open trade REST implementation exposes offer taking and
PATCH trade events; those mutation endpoints are intentionally excluded
from this observer. It uses authenticated WebSockets because the open
trade list and changing state reside on TRADES and TRADE_PROPERTIES.

For source-grounded regression tests without a Bisq checkout:

~~~bash
python -m unittest discover -s tests -p 'test_bisq_pair.py' -v
python -m unittest discover -s tests -p 'test_regtest.py' -v
~~~

The tests cover QR framing, credential protection, loopback restrictions,
WebSocket subscription envelopes, state convergence, wrong receiving
amount, premature chain credit, and isolated process launch preflight.
An optional manual GitHub Actions workflow builds upstream Bisq and boots
the pair. Its costs and success depend on the upstream build/environment.
Automated CI checks the adapter boundary without building upstream Java.

## Upstream references

- https://github.com/bisq-network/bisq2/blob/main/scripts/local-3node.sh
- https://github.com/bisq-network/bisq2/blob/main/apps/api-app/src/main/resources/api_app.conf
- https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/rest_api/endpoints/access/AccessApi.java
- https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/web_socket/domain/trades/TradesWebSocketService.java
- https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/web_socket/domain/trades/TradePropertiesWebSocketService.java
- https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/web_socket/subscription/SubscriptionPermissionMapping.java
- https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/access/pairing/qr/PairingQrCodeEncoder.java
- https://github.com/bisq-network/bisq2/blob/main/common/src/main/java/bisq/common/util/BinaryEncodingUtils.java
