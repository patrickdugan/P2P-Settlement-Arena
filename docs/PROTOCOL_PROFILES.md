# Protocol profiles and source fidelity

The CLI's default profile is \`bisq-easy\`, following the current Bisq 2 accessible, payment-first mode. The original Rust settlement fault-injection engine is retained as \`bisq-musig-model\`.

These are independent **behavioral models**, with explicit simplifying assumptions.

## Default: Bisq Easy

**Architecture:** the BTC buyer pays fiat to the seller first. The seller releases BTC afterward. Bisq Easy relies on seller reputation rather than locking both traders' security deposits. For this reason the default model has *zero escrow funds* and preserves separate fiat and BTC balance conservation.

**Source:** https://bisq.network/blog/trade-protocols/

### Offer and risk-control rules implemented

- Fixed price quotes in **USD only**. Reject below $6 and above $600.
- Seller reputation score of at least 1,200 for creating a sell offer. Accept amounts requiring 200 reputation points per rounded USD, with the current rule's greater of 5% (rounded) or 200 additional points. This mirrors the acceptance tolerance in the 2026 Bisq 2 specification, though a real client's displayed offer maximum uses the lower, unbuffered threshold.
- Fail closed when market prices are absent, at least 12 hours old, or their observation timestamps differ by more than 12 minutes.
- Contract price specification must match the offer's fixed price and the quoted BTC amount must be consistent with that fixed price.
- Buyer payment claims are distinct from **actual bank transfer events**. The seller can only confirm when the model's external ledger shows fiat received.
- The BTC payment request's amount is either unspecified or equal to the contract amount, mirroring one required property of Bisq's real payment-data validator.
- Any confirmed on-chain BTC send pays a synthetic 300-sat miner fee. There is no BTC escrow or intermediary.
- Logical deadlines identify seller non-delivery. The 3-round benchmark deadline is **an arena-defined test knob**, not Bisq's trading-period specification.
- An optional synthetic reversible fiat rail allows us to exercise chargeback economics, even though real Bisq restricts payment methods according to chargeback risk.

**Source specs:**

- https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-seller-reputation-limit.md
- https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-contract-price.md
- https://github.com/bisq-network/bisq2/blob/main/docs/specifications/trade/bisq-easy-bitcoin-payment-data.md

### Important exclusions

This simulator currently lacks cross-currency BTC/USD price conversion, actual provider signatures, cryptographic contract hashes, user-network IDs, social reputation proofs, real payment-method rules, Lightning/on-chain payment-data parsers, Tor/P2P messaging, mediation, Bisq v1 wallet compatibility, and live trade execution.

The \`SetBitcoinPaymentRequest\` event models its requested amount as an already-parsed \`Option<u64>\`. Real BOLT 11/BIP 21 parsing requires robust handling of percent-encoded strings and alternate wallet interpretations, which must be added before claiming payment-data parser coverage.

## Experimental: Bisq MuSig-inspired

**Architecture:** synthetic Bitcoin escrow, abstract fiat-payment state machine, durable-in-model event journal, ACK policy, restart, and watchtower warning response. This profile intentionally includes an unsafe ACK-first baseline to produce a proof-of-concept state-loss trace.

**Sources:**

- https://github.com/bisq-network/bisq-musig/tree/main/protocol
- https://github.com/bisq-network/bisq-musig/blob/main/concept/watchtower/OverviewWatchTowers.md
- https://github.com/bisq-network/bisq2/issues/5126

The real Bisq MuSig protocol involves cooperative Taproot signing and staged claim/penalty paths. The arena's escrow abstraction does not reproduce actual transactions, keys, receipts, or peer key exchange. The journal survives process restarts **by simulation contract** and is not a filesystem write-ahead log.

## Interoperability path

\`CLI -> protocol profile -> reference model -> event journal -> assertion/scorer\`

Future adapters can replace the reference model with a sandboxed Bisq regtest node over well-defined interfaces while preserving the scenario inputs. External integration will require license review and isolation of source-derived interfaces from the independent MIT simulator.
