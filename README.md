# P2P Settlement Arena

A deterministic, adversarial **P2P settlement protocol laboratory** for Red Team Gladiatorics (RTG). Initial focus: the crash-consistency, protocol-ordering, replay, and watchtower failure modes seen in decentralized Bitcoin/fiat exchange.

This repository starts with a **dependency-free Rust reference state machine**. Everything runs with simulated money, actors, time, and process crashes. No private keys, actual BTC, external wallet, blockchain, fiat API, or exchange integration.

## Quick start

Requires a stable Rust toolchain.

~~~sh
cargo test
cargo run -- list
cargo run -- run ack-before-commit
cargo run -- all > arena-results.jsonl
~~~

The CLI prints one JSON object per scenario. Example fields:

~~~json
{"scenario":"ack-before-commit","passed":true,"attack_success":true,"recovered":true,"trials":1,"committed_events":0,"acknowledgements":1,"final_phase":"Offered","safety_findings":["ack_without_durable_commit"]}
~~~

Here, **passed** means the benchmark produced its expected result. **attack_success** reports a demonstrated control failure. The deliberately unsafe ACK-first implementation creates a minimal counterexample; the safe model uses commit-before-ACK.

## Model

- **Roles:** seller, buyer, mediator, watchtower, each with action authorization.
- **Trade lifecycle:** offer → escrow lock → fiat marked sent → fiat confirmed → release, with dispute mediation and a timeout refund path.
- **Accounting:** 100,000 simulated sats and a 300-sat settlement fee. Buyer, seller, escrow, and miner balances must conserve total value.
- **Journal:** a receiver applies a candidate transition, appends the accepted event to its modeled durable journal, then acknowledges. Identical message IDs replay idempotently; altered payloads with the same ID are rejected.
- **Failure injection:** crash immediately before commit or after commit but before ACK; restart rebuilds state from journal.
- **Watchtower:** a post-settlement warning starts a logical response window. A missed window is surfaced as a safety finding. This models observation requirements; it does not execute a Bitcoin penalty transaction.
- **Peers:** the reconciliation scenario delays events at one peer and later checks convergent states and event histories.

The modeled durability invariant is:

~~~text
ACK(message) => Durable(Applied(message))
~~~

The accounting invariant is:

~~~text
seller_sats + buyer_sats + escrow_sats + miner_sats == 100_000
~~~

## Benchmark scenarios

| Scenario | Expected result |
| --- | --- |
| baseline | Successful settlement |
| duplicate-replay | One durable effect, repeat ACK |
| out-of-order | Reject early event, accept retry in order |
| crash-before-commit | Restart and retry, then settle |
| crash-after-commit | Replay durable event, ACK retry, then settle |
| ack-before-commit | Intentional ACK-without-commit vulnerability detected |
| conflicting-message-id | Modified replay rejected |
| unauthorized-release | Role enforcement rejects buyer release |
| double-settlement | Second payout rejected |
| dispute-resolution | Mediator settles one disputed escrow |
| watchtower-success | Warning countered in window |
| watchtower-timeout | Missed response window detected |
| fault-sweep | 8 deterministic injection placements |
| peer-reconciliation | Delayed peer converges after delivery |

Run the complete set with \`cargo run -- all\`. A machine-readable JSONL report can later become a ControlArena scorer input.

## Boundaries

This is a **model**, not a production exchange or Bitcoin implementation. A modeled durable journal is an in-memory value that survives the simulated process restart; it is not an fsync-backed write-ahead log. The logical clock is controlled by an abstract mediator. Fiat confirmations and mediation decisions are opaque trusted events. The model does not implement MuSig, Taproot, adaptor signatures, payment rails, gossip, cryptographic ownership checks, reorgs, or real network concurrency.

The alert and response sequence approximates a Bisq-style post-settlement warning risk. The simulated watchtower cannot prove a real penalty transaction would be valid or timely.

## Research lineage

The initial research targets were inspired by ongoing reliability discussions in [Bisq 2](https://github.com/bisq-network/bisq2), [bisq-musig](https://github.com/bisq-network/bisq-musig), including [stuck trade follow-ups](https://github.com/bisq-network/bisq2/issues/5126) and [MuSig watchtower concepts](https://github.com/bisq-network/bisq-musig/blob/main/concept/watchtower/OverviewWatchTowers.md). Those upstream projects are independent; this code does not copy their protocol implementations.

See [threat model](docs/THREAT_MODEL.md) and [ControlArena adapter sketch](docs/CONTROLARENA.md).

## Next experiments

1. Replace the model journal with a disk-backed append, checksum, and fsync boundary; inject kill/restart between actual disk operations.
2. Introduce separate sender/receiver outboxes, delivery receipts, retransmission, mailbox expiry, and adversarial scheduling.
3. Add an adapter to execute Bisq-like regtest scenarios through a narrow wallet/chain trait interface.
4. Export traces, scoring artifacts, and seeded variations as a custom AISI/Redwood ControlArena Setting.
5. Add agent strategies that bid for liquidity, attempt settlement fraud, and operate a defender/watchtower under explicit budgets.
