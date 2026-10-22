# Bitcoin Core regtest and Bisq 2 observation

This work adds opt-in, separate external adapters around the independent
Bisq Easy-inspired P2P Settlement Arena:

- A real Bitcoin Core **regtest-only** payout and reorg verification driver.
- A strictly GET-only, localhost-only observer for a locally paired Bisq 2 API.

The BTC chain transactions are real **regtest transactions**, worth no mainnet
money. The fiat receipt continues to be synthetic and comes from the Rust
ControlArena model. The chain driver uses Core's isolated wallets; it does not
execute a Bisq trade, access Bisq wallets, or use Bisq payment credentials.

## 1. Start a disposable Bitcoin Core regtest daemon

The sample uses the **unofficial** bitcoin/bitcoin:31.1 Docker testing image,
which is not endorsed by the Bitcoin Core project. A verified local bitcoind
release is also supported if it exposes the same RPC on loopback.

~~~bash
docker run -d --rm --name rtg-bitcoin-regtest \
  -p 127.0.0.1:18443:18443 \
  bitcoin/bitcoin:31.1 \
  -regtest=1 -server=1 -listen=0 -dnsseed=0 -connect=0 \
  -rpcbind=0.0.0.0 -rpcallowip=0.0.0.0/0 -rpcport=18443 \
  -rpcuser=rtg -rpcpassword=regtest-only-local \
  -fallbackfee=0.0002 -txindex=1 -printtoconsole=1
~~~

The RPC is bound to all container interfaces to allow Docker-host access.
The published host port is bound ONLY to 127.0.0.1, and the container must
never be publicly port-forwarded. Stop it with:

~~~bash
docker stop rtg-bitcoin-regtest
~~~

## 2. Drive a real regtest transaction

~~~bash
cargo build
export P2P_REGTEST_RPC_USER=rtg
export P2P_REGTEST_RPC_PASSWORD=regtest-only-local
python -m controlarena_adapter.bitcoin_regtest \
  --wallet-prefix arena01 --wait-seconds 90 --reorg
~~~

On Windows PowerShell, set the variables using the usual
$env:P2P_REGTEST_RPC_USER = "rtg" and
$env:P2P_REGTEST_RPC_PASSWORD = "regtest-only-local".

The driver:

1. Obtains strict-model approval for the synthetic paid-fiat trade.
2. Checks Core's chain name **regtest** and the canonical regtest genesis
   block hash 0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206.
3. Creates two fresh named descriptor wallets. Existing wallet names cause
   the run to fail; choose a new wallet prefix for subsequent runs.
4. Mines 101 regtest blocks to mature a seller coinbase reward.
5. Sends precisely 100,000 sats to the new buyer regtest address.
6. Checks decoded output amount, buyer wallet receipt, miner fee and at least
   one confirmation.
7. When --reorg is provided, invalidates the payout block and mines a
   replacement, requiring reconfirmation of the same payout.
8. Prints a JSON report containing chain, transaction ID, confirmed amount,
   satoshi fee, the reorg result and the synthetic authorization result.

Real balances are checked by Bitcoin Core. The fiat side remains modeled.
Coinbase issuance during the fixture makes aggregate wallet deltas unsuitable
for simplistic value conservation checks; the payout output and fee are
verified individually instead.

## 3. Safety boundaries

The RPC URL accepts only http://127.0.0.1:<port> with explicit credentials.
The adapter disables proxies and rejects URL credentials, remote addresses,
non-regtest chain names and wrong regtest genesis fingerprints **before**
creating wallets or broadcasting. Recipient addresses must begin with bcrt1.

Each run creates new wallet names. No real Bitcoin network, testnet, signet,
or production wallet is enabled by this adapter. The RPC Basic credentials
are for local testing only.

## 4. Read-only local Bisq 2 market observation

The source of the current Bisq 2 API exposes REST resources:

- GET /api/v1/market-price/quotes
- GET /api/v1/offerbook/markets/{currencyCode}/offers
- GET /api/v1/trades/closed (explicitly opt in)

With your independently running, authorized Bisq 2 node on port 8090:

~~~bash
python -m controlarena_adapter.bisq_observer --market USD
~~~

If the node requires a paired client session:

~~~bash
export BISQ_CLIENT_ID=YOUR_CLIENT_ID
export BISQ_SESSION_ID=YOUR_SESSION_ID
python -m controlarena_adapter.bisq_observer --market USD
~~~

Headers are sent under the names Bisq-Client-Id and Bisq-Session-Id,
matching the upstream Java API. This adapter uses GET only, never takes
offers or mutates trades. The returned JSON contains counts and availability
flags; raw counterparties, conversations, payment data and closed trades are
never printed. To count your own closed trades explicitly, use --closed-trades.

## 5. CI and limitations

The bitcoin-regtest GitHub Actions job runs Python safety-unit tests and an
actual 31.1 bitcoind regtest container, tests a wallet payout with an injected
one-block reorg, then stops the disposable container. The report is uploaded
as a JSONL artifact. All regtest activity is opt-in.

A future true Bisq-to-regtest integration needs two separately configured
Bisq 2 processes, paired API clients, a mocked fiat/payment rail, and trade-ID
to actual chain-payout correlation. The current system only observes Bisq via
read-only API and executes Bitcoin Core payouts separately.

Upstream references:
https://github.com/bisq-network/bisq2/blob/main/api/architecture.md
https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/rest_api/endpoints/offers/OfferbookRestApi.java
https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/rest_api/endpoints/trades/TradeRestApi.java
https://github.com/bisq-network/bisq2/blob/main/api/src/main/java/bisq/api/access/filter/Headers.java
https://github.com/bisq-network/bisq-musig/blob/main/testenv/README.md
https://bitcoin.org/en/releases/31.1/
