# Seeded Bisq Easy multi-agent market

The `swarm` simulator adds many budget-constrained buyers and sellers to the
existing independent MIT-licensed Bisq Easy reference model. All accounts,
prices, fiat events, BTC inventory, and settlement outcomes are synthetic.
It is designed for reproducible adverse-condition testing with zero network access.

## Architecture

`seed -> participants -> resting offers -> buyer selection -> EasyNode per-trade
state machine -> shared balance ledger -> JSONL trace and scorer`

One seller has one active offer at a time; an offer is consumed by at most one
buyer and expires after 4 logical rounds. A seller may replenish the resting
offer after it is taken or expires. Exactly one buyer is randomly selected per
round. It is a sequential book, with no concurrent locks, competing transaction
confirmation races, partial fills, fiat credit risk, or price discovery from real
BTC exchanges.

Each accepted offer calls the existing `Offer::verify` to enforce Bisq Easy's
reputation acceptance limit, price freshness, fixed contract price, and
synthetic inventory bound. `EasyNode` executes the same buyer-first fiat event
sequence and BTC delivery rules as the single-trade benchmark. Afterward the
arena updates the *shared* seller and buyer balances with the trade's actual
state changes.

Independent global conservation checks run after every trade:

```text
sum(all buyer fiat) + sum(all seller fiat) = initial total fiat
sum(all buyer BTC) + sum(all seller BTC) + miner fees = initial total BTC
```

A trade record carries stable agent indices, offer ID, logical round,
assigned strategies, synthetic quote, simulated payout, blocked attempts,
and capital at risk. This is not an actual P2P wire protocol trace.

## Policies

`reputation-first` (default): restrict buyer selection to offers with at
least 2x the reputation coverage needed for the quoted USD value. It also
chooses only the synthetic irreversible payment rail. Rank qualifying
listings by reputation, then price. This is an **arena policy**, above and
beyond current Bisq Easy's protocol acceptance tolerance.

`best-price`: rank eligible offers by lowest fixed USD/BTC price, using
seller reputation and then offer ID as tie-breakers. The standard
`Offer::verify` validation still applies; rejected offers are withdrawn and
counted. Reversible rails here are *artificial adversarial inputs*.

Both approaches use the same generated population. Buyer arrivals are fixed per round, and each seller's candidate quote and price are fixed by `(seed, round, seller_id)`. Thus offers do not consume a shared random stream. Resulting inventory, active offers, and accepted matches can diverge between policies.

## Buyer and seller strategies

Buyer behavior (one stable strategy per simulated buyer):
- Honest: fiat transfer, proof notification, approved BTC request.
- False payment claim: notifies seller without the bank event, which fails
  `ConfirmFiat` and keeps the seller's BTC.
- Oversized invoice: asks for ten times the contract BTC amount. The
  reference-model validator rejects it before the buyer submits the permitted
  amount and the seller sends BTC.
- Reversible-rail chargeback: after BTC delivery, the synthetic bank tries
  to reverse fiat. An irreversible rail blocks the attempt; otherwise
  `post_delivery_chargeback` records seller exposure.

Seller behavior:
- Honest: sends the contracted BTC after confirmation.
- BTC withholder: receives fiat, lets a synthetic 4-round delivery deadline
  elapse, and the buyer enters dispute. `seller_non_delivery` records the
  fiat exposure, **without** pretending that mediation recovers a payment.

The 4-round response is an arena knob; it is not a Bisq deadline.

## Example CLI

~~~bash
cargo run -- swarm
cargo run -- swarm --seed 18 --rounds 200 --buyers 14 --sellers 10 --attacker-pct 25
cargo run -- swarm --seed 18 --rounds 200 --buyers 14 --sellers 10 --attacker-pct 25 --policy price --trace > prices.jsonl
cargo run -- swarm --seed 18 --rounds 200 --buyers 14 --sellers 10 --attacker-pct 25 --policy reputation --trace > reputation.jsonl
cargo run -- swarm-compare --seed 18 --rounds 200 --buyers 14 --sellers 10 --attacker-pct 25 --trace > paired.jsonl
~~~

`--trace` writes JSON objects of `kind=trade` (including policy) plus a final
`kind=swarm_summary` object. All runs report exact integer USD cents and
Bitcoin satoshis; there is no floating-point valuation.

`attacker_percent` describes the share of agents randomly assigned an
adversarial strategy. It is an input, not a measured share of attacks or
successful exploits. A default seed is provided to make examples repeatable.
Choose many seeds to avoid ranking policies on one generated population. `swarm-compare` outputs two policy summaries and a `kind=policy_comparison` record with signed best-price-minus-reputation differences. Both trials share external arrival and offer RNG streams, while inventory and resting-book availability respond to their distinct policy choices.

## Interpret the scoreboard carefully

- `completed` counts BTC deliveries, including those followed by a synthetic
  chargeback. `post_delivery_chargeback` is a subset of those completions.
- `attack_successes` counts seller non-delivery and post-BTC chargeback
  outcomes. It is an in-model scenario result, not evidence of a live exploit.
- `fiat_volume_cents` counts gross fiat transfers, including payments later
  reversed by synthetic chargebacks. Account balances show the net reversal.
- `blocked_attacks` counts attempted false fiat confirmations, oversized BTC
  payment data, and forbidden reversals.
- `buyer_usd_at_risk_cents` is the sum of fiat paid without contracted BTC
  after the synthetic response deadline.
- `seller_btc_at_risk_sats` counts BTC sent when synthetic fiat was reversed,
  with the modeled miner fee.
- `conserved=true` checks both currencies independently across all agents.
  Economic losses can occur while these ledger conservation checks hold.

## Remaining realism gaps

Bisq Easy is peer-negotiated rather than a centralized order book. This
synthetic book is a discovery and scheduling abstraction. The marketplace
does not currently model reputation proofs or sybil networks, cross-currency
quotes, actual fiat payment method limits, adversarial oracle responses,
multiple simultaneous escrow commitments, BIP 21/BOLT 11 parsing, reputation
decay, market maker inventory hedging, real payment-finality windows, or
multi-threaded message delivery.

The scenario score only captures *synthetic* risk and safety outcomes.
Use a dedicated sandbox and license review before connecting any real Bisq
source code or regtest binaries.

## Upstream reference material

- [Bisq Easy reputation limit](https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-seller-reputation-limit.md)
- [Bisq Easy contract price binding](https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-contract-price.md)
- [Bisq Easy BTC payment data checks](https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-bitcoin-payment-data.md)
