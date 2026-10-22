# Threat model

All value is synthetic. No real wallet operations, P2P network actions, payment credentials, fiat accounts, or Bitcoin signatures are present.

## Bisq Easy (default)

**Actors:** buyer agent, seller agent, bank oracle, deterministic clock.

**Protected assets:** buyer's fiat before BTC delivery, seller's BTC once payment is confirmed, market price specification, reputation-score limits, payment request amounts, and trade event integrity.

**Threat actions:**

- Seller accepts fiat and delays/refuses delivery beyond the benchmark's logical deadline.
- Buyer submits a BTC request whose amount is greater than the agreed trade amount.
- Buyer claims payment without external fiat transfer.
- Buyer attempts to renegotiate a signed fixed price by altering the taker's contract.
- Counterparty tries to use stale or mismatched market observations to bypass reputation limits.
- Counterparty replays an event ID with new contents or duplicates a valid event.
- Synthetic bank reverses an already completed fiat payment over an adversarial reversible rail.

The bank oracle, clock, and role labels are trusted for this version. Seller score is static, and actual bank, wallet, and reputation authenticity are out of scope.

The simulator conserves total BTC sats and total fiat cents independently. Its 3-round delivery deadline is a benchmark parameter, not a Bisq network parameter.

## MuSig-inspired (secondary)

**Protected assets:** abstract escrow, settlement recipient, journaled effects, message IDs, watchtower visibility.

Attacker controls: order and duplicate/mutation of messages, crash position relative to commit, role spoof attempts, and an unobserved warning deadline.

Invariants:

- \`ACK(event) => journal contains accepted event\`.
- Replay does not change a committed payload.
- Exact integer BTC balance conservation and a single terminal payout.
- Crash recovery reproduces the committed state.
- Warning responses within the modeled observation window prevent a missed deadline.

The abstract \`Vec<Event>\` journal survives simulated crashes. No fsync semantics are claimed. Mediation in the old model is intentionally stronger than real Bisq v1 or MuSig mediation and must not be used as a specification for production fund control.

## Negative controls

Scenarios \`seller-withholds\`, \`chargeback-after-delivery\`, \`ack-before-commit\`, and \`watchtower-timeout\` deliberately construct adverse outcomes. A passing regression test means the evaluator detected the modeled outcome. The result establishes no live Bisq vulnerability.

Cross-currency conversion, real BOLT 11/BIP 21 parsers, chain reorgs, nonce reuse, cryptographic authentication, real customer losses, slippage, reputation gaming, and multi-trade capital allocation belong to future experimental phases.
