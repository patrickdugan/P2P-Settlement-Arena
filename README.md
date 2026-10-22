# P2P Settlement Arena

A deterministic, MIT-licensed Rust laboratory for adversarial evaluation of peer-to-peer Bitcoin/fiat settlement, developed for Red Team Gladiatorics (RTG).

**Default market: Bisq 2 / Bisq Easy.** The main simulator models its small-trade, payment-first flow with seller reputation and no escrow. A separate **Bisq MuSig-inspired** profile preserves the original abstract escrow, journal, crash, and watchtower tests. Both are independent simulations, not integrations with live Bisq nodes.

This is a research benchmark with *simulated* BTC and fiat. No wallets, external payment providers, crypto keys, transaction broadcasting, real funds, or user accounts are connected.

## Run it

Requires stable Rust.

~~~bash
cargo test
cargo run --                                   # Bisq Easy honest exchange
cargo run -- list
cargo run -- all                               # every Bisq Easy scenario
cargo run -- simulate seller-withholds
cargo run -- simulate --profile bisq-musig all # MuSig-inspired fault sweep
cargo run -- profiles
~~~

CI checks formatting, Clippy, Rust tests, and the entire scenario suite for **both** profiles. It uploads JSONL results.

### Sample: seller withholds BTC after fiat payment

~~~json
{"protocol":"bisq-easy","scenario":"seller-withholds","passed":true,"attack_success":true,"blocked_attacks":0,"committed_events":8,"final_phase":"FiatConfirmed","buyer_usd_at_risk_cents":5000,"seller_btc_at_risk_sats":0,"safety_findings":["seller_non_delivery"]}
~~~

Here, \`passed\` means the benchmark matched its oracle, while \`attack_success\` means the model demonstrated a forbidden economic outcome. Some tests intentionally show a loss even when the benchmark passes. Each result is one JSON line.

## Bisq Easy profile, the default

Bisq 2's Easy flow has the **buyer pay fiat first**, then the **seller send Bitcoin**. Its principal defense against a seller withholding BTC is reputation. Our fixed-price USD simulator mirrors the order of actions and selected checks documented by Bisq:

- **Offer acceptance:** trade USD value between $6 and $600; seller's reputation must cover the trade with a score of 200 points per rounded USD and the documented 5% / 200-point acceptance tolerance.
- **Market oracle:** required price must exist, be younger than 12 hours, and agree with the USD price timestamp within 12 minutes.
- **Contract binding:** the taker's requested fixed-price specification must exactly match the maker's offer.
- **Fiat transfer:** modeled as a separate external bank event; a buyer saying they paid cannot make the seller's node treat that claim as receipt.
- **Bitcoin payment data:** abstract requested amount must be unspecified or exactly the contract's BTC amount; the seller enforces this before marking payment data approved.
- **Bitcoin delivery:** seller sends the exact BTC amount to buyer, paying a fixed synthetic 300-sat miner fee from its own inventory. The simulation has **no escrow**.
- **Economic outcomes:** seller non-delivery after payment and a hypothetical reversible-fiat chargeback after BTC delivery are reported as attacks in the model.

The real Bisq implementation validates BOLT 11/BIP 21 payment data through multiple encodings and supports more payment types and quote currencies. Our abstract requested-amount model is narrower, and the reversible rail is explicitly a **hypothetical risk-injection setting**, not a claim that Bisq supports a particular reversible payment service.

### Bisq Easy benchmark cases

| Scenario | What it exercises | Result expected |
| --- | --- | --- |
| honest-trade | Fiat payment first, BTC delivered afterward | Settlement |
| seller-withholds | Seller receives fiat, withholds BTC past deadline | Exposure observed |
| chargeback-after-delivery | Synthetic reversal of completed fiat payment | Seller exposure observed |
| invoice-overpay | Buyer proposes BTC payment request for 10x amount | Request blocked |
| price-spec-tamper | Taker mutates agreed fixed price | Offer rejected |
| low-reputation | Seller score insufficient for offered amount | Offer rejected |
| stale-price | Market observation reaches 12 hours old | Offer rejected |
| oracle-skew | Fiat market observations differ by more than 12 minutes | Offer rejected |
| fake-fiat-notice | Buyer claims payment without actual transfer | Confirmation blocked |
| duplicate-replay | A message ID is repeated identically | Idempotent |
| conflicting-replay | A committed message ID is reused with another event | Mutation rejected |
| irreversible-rail | Synthetic irreversible bank rail resists reversal | Chargeback blocked |

## Bisq MuSig-inspired profile

The existing core models a **different** settlement mechanism with escrow balances, message durability, mediation, crash injection, and a warning/response window.

~~~bash
cargo run -- simulate --profile bisq-musig list
cargo run -- simulate --profile bisq-musig ack-before-commit
cargo run -- simulate --profile bisq-musig fault-sweep
~~~

Its intentionally unsafe ACK-first mode demonstrates a violation of:

~~~text
ACK(message) => Durable(Applied(message))
~~~

This profile uses a synthetic escrow model and a simulated watchtower. It does **not** implement actual MuSig signing, Taproot, PSBTs, adaptor secrets, staged transaction payouts, a wallet, regtest, or Bitcoin consensus. Calling it MuSig-inspired identifies the research target rather than protocol fidelity.

## Source boundaries

- [Bisq trade protocol overview](https://bisq.network/blog/trade-protocols/) (Bisq Easy vs MuSig architecture)
- [Bisq Easy seller reputation requirements](https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-seller-reputation-limit.md) (trade caps, score tolerance, price freshness)
- [Bisq Easy price-specification binding](https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-contract-price.md)
- [Bisq Easy Bitcoin payment data validation](https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-bitcoin-payment-data.md)
- [Bisq MuSig watchtower concept](https://github.com/bisq-network/bisq-musig/blob/main/concept/watchtower/OverviewWatchTowers.md)
- [Bisq 2 stuck trade follow-ups](https://github.com/bisq-network/bisq2/issues/5126)

No upstream implementation source is copied. The models are MIT-licensed and have no third-party Rust dependencies.

See [Protocol Profiles](docs/PROTOCOL_PROFILES.md), [Threat Model](docs/THREAT_MODEL.md), and [ControlArena Adapter](docs/CONTROLARENA.md).

## Next upgrades

1. Integrate deterministic seeded offer books and market making agents with multiple trading counterparties.
2. Add cross-currency markets and reproduce price conversion, rounding, and offer limits for supported Bisq payment methods.
3. Add a strict parser for payment request formats and multiple layers of percent-decoding for BOLT 11 and BIP 21 safety checks.
4. Develop an fsync-backed journal and simulated network/mailbox scheduling for the MuSig-inspired profile.
5. Build a custom Setting for the AISI/Redwood ControlArena framework, connecting simulated attacker tools and scoring outputs.
