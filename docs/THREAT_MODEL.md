# Threat model and invariants

## Assets

Simulated escrow, the recipient of a settlement, durable event history, message identity, and visibility of time-sensitive warnings.

## Attacker powers in v0

The harness may delay, replay, mutate, or reorder an event; choose crash boundaries around commit; impersonate a role at the protocol-message layer; and allow a watchtower deadline to pass. The "unsafe ACK" policy intentionally violates durability to create a counterexample.

A test can control these knobs without accessing a genuine peer, chain, wallet, bank account, or user identity.

## Controls

| Property | Defender check | Adversarial trial |
| --- | --- | --- |
| Conservation | Exact integer balances and escrow accounting | Double settlement |
| Authorization | Each action belongs to an actor role | Buyer attempts release |
| Replay integrity | Stable ID implies identical payload | Conflicting message ID |
| Idempotence | A duplicate has no second effect | Duplicate replay |
| Commit ordering | ACK implies journaled state transition | ACK-first crash |
| Recovery | Rebuild state from ordered committed events | Crash before/after commit |
| Liveness | Stalled events require retransmission | Out-of-order delivery |
| Convergence | Peers eventually agree after complete delivery | Delayed peer event |
| Watchtower deadline | Detect missed response after warning | Watchtower timeout |

## Modeling assumptions

- Durability is abstract: a journal vector survives simulated crashes. OS power loss, torn writes, snapshots, and filesystem caches are outside v0.
- Every message carries an integer ID and a role; neither is authenticated cryptographically.
- A mediator controls the logical clock and dispute resolution. The model makes no claim about impartiality, fiat-payment truth, or fraud economics.
- The warning is an external modeled event. A timely counter-warning is a state-machine response, not a valid Bitcoin penalty signature or broadcast.
- Fees and trade size are fixed model parameters. Bitcoin transaction weight and feerate changes are outside v0.
- There is one trade per Node in this release.

## Attack success and benchmark success

\`passed=true\` means a scenario matched its test oracle. For negative controls, \`attack_success=true\` means the fault became observable in the intentionally vulnerable model, such as an ACK without a committed event. A detector reporting a fault is evidence of model behavior under an assumption, not evidence of a bug in Bisq or any live service.

## Research priorities

For storage, model fsync of journal entries and an atomic receipt/outbox handoff. For settlement, add UTXO ownership, pre-signed transaction outputs, fee bounds, signer nonces, and an abstract chain that can reorg. For agent control, measure attacker cost, defender false positives, funds at risk, liveness, and competing benign workload throughput.
