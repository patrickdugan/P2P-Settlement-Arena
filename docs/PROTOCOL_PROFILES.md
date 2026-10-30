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

## Cashout: BTC to a third-party fiat payee (BAN contract `ban-arena-cashout/1`)

**Architecture:** a sender S pays BTC toward a handle; a liquidity provider P that **buys** the BTC pays a recipient R, who is not the seller, through a modeled fiat rail. The contract is [Bisq-Agent-Network `src/ban/arena_contract.py`](https://github.com/patrickdugan/Bisq-Agent-Network/blob/2ae98c6b9e2fcd16df4af21eb175504120565177/src/ban/arena_contract.py) @`2ae98c6` (Cash App Global architecture, spec D.5). Everything is synthetic: no keys, rails, nodes or funds.

| Cashout actor | Arena role | Why |
| --- | --- | --- |
| Provider P (maker BUY offer) | `buyer` | P buys BTC and pays fiat; never the seller (a compile-time assertion) |
| Sender S | `seller` | S delivers BTC |
| Recipient R | `fiat_payee` | Fiat goes to a third party, not the seller |

Two profiles, kept separate because the exposure falls on opposite parties. The party that moves first bears the second mover's misbehavior; misbehavior by the first mover or the environment is blocked by the other side's checks.

| Profile | Order | Exposure field |
| --- | --- | --- |
| `cashout-btc-first` (`src/cashout.rs`) | S funds P's fresh deposit script; P accepts it from its own chain view after `min_conf`; P writes one payout authorization; the rail credits R; P issues a receipt | `sender_btc_at_risk_sats` |
| `cashout-bisq-easy` (`src/cashout_easy.rs`) | The Bisq Easy machine with seller = S and buyer = P: P pays R first; S confirms only on R's acknowledgment or rail evidence; S sends BTC; P accepts the output from its own chain view | `provider_minor_at_risk` |

### Behaviors and oracles

The oracle table equals BAN's `BEHAVIORS`; a test checks it against the vendored BAN fixture.

| Behavior | `cashout-btc-first` | `cashout-bisq-easy` |
| --- | --- | --- |
| `honest` | settled | settled |
| `provider_withholds` | exposure observed: P keeps S's deposit unpaid past the deadline | blocked: P never pays, S never releases |
| `sender_withholds` | blocked: S never funds, the quote expires | exposure observed: S keeps its BTC after R was paid (Bisq Easy `seller-withholds`) |
| `replay_funding` | blocked: an output of an earlier quote is refused (used outpoint, below the quote's height) | exposure observed: P refuses the replayed output, but S still never delivers |
| `wrong_output` | blocked: an output paying S's own script is refused | exposure observed: likewise |
| `late_funding` | blocked: funding after expiry is never paid at the old rate and is refunded | exposure observed: S delivers only after the deadline (`seller_late_delivery`) |
| `duplicate_payout` | blocked: one authorization per quote, one rail credit per key | blocked: likewise |
| `rail_timeout` | blocked: no resubmission while the outcome is unknown; resolved only by a rail status query | blocked: likewise; S confirms on rail evidence |
| `claim_without_credit` | exposure observed: the receipt is refused without a rail credit, but S's BTC stays held | blocked: S never confirms on P's claim |

`stale_chain`, `substituted_provider_key` and `alias_equivocation` are BAN-side checks (chain tip, key pinning, alias resolution) and are not modeled; `cashout-import` skips them by name.

### Rules implemented

- R is credited only by `RailCredit` from the rail actor. P's `ClaimPaid` is a statement: in btc-first a receipt needs an observed rail credit; in bisq-easy S's confirmation needs R's acknowledgment or rail evidence, and neither exists without a credit.
- S's BTC counts only as an output in P's chain view: unused outpoint, at or above the quote's height, P's deposit script, at least the quoted value, at most the expiry height (btc-first only), and `min_conf` confirmations.
- One payout authorization per quote, written before the rail call; one credit per idempotency key. A timeout leaves the payout unknown until a status query; a status without a credit closes the key and the deposit is refunded.
- BTC (sender, provider deposit, miner) and fiat minor units (provider float, recipient) are each conserved with checked integer arithmetic, and every node replays from its journal.
- `*_at_risk` fields report the largest value left unsecured past a deadline during the run. In bisq-easy a late delivery cures the debt but not the exposure, which is reported as `seller_late_delivery`.
- Result lines carry exactly BAN's cashout key set: `protocol`, `scenario`, `passed`, `attack_success`, `blocked_attacks`, `committed_events`, `final_phase`, `sender_btc_at_risk_sats`, `provider_minor_at_risk`, `recipient_credited_minor`, `currency`, `exponent`, `safety_findings`. BAN's `parse_arena_line` rejects the Bisq Easy keys `buyer_usd_at_risk_cents` and `seller_btc_at_risk_sats` on these protocols, so they are omitted.

### Assumptions and exclusions

- **Arena knobs.** The 3-block payout deadline (btc-first), the 6-block quote lifetime used for imported scenarios, and the reused Bisq Easy 3-round delivery deadline are test knobs, not rail or Bisq properties.
- **Sender reputation.** `cashout-bisq-easy` keeps Bisq Easy's seller-reputation gate, which here is the *sender's* reputation (fixed at 120,000 points, enough for $600). BAN's senders sign with ephemeral keys and have none, so in practice that profile leaves P's risk uncapped.
- **Honest recipient.** R acknowledges only a credit it received. A colluding R that acknowledges falsely is out of scope; acknowledgments are gameable (spec G.10).
- **Currencies.** `cashout-btc-first` never converts currency, so it runs any ISO 4217 code in its own minor units. `cashout-bisq-easy` reuses the USD-only Bisq Easy machine ($6–$600), so it honors BAN's `arena_supported` and refuses other currencies. The vendored BAN fixture is in ILS: its btc-first lines run on BAN's own amounts, and its bisq-easy lines are skipped.
- **Not modeled.** Scenario IDs (`SHA256("BAN-ARENA-SCENARIO-V1\0" ‖ TL-Canon bytes)`) are not recomputed; a ControlArena cashout Setting is not built; real rails, receipts, signatures and chain data are absent.

### Importing BAN scenarios

`cargo run -- cashout-import fixtures/ban/cashout_scenarios.jsonl` parses each `ArenaScenarioV1` strictly (exact keys, integers only, contract and source checked, the deviating actor matching BAN's table, `arena_supported` matching the USD rule), runs it in the matching profile on its own amounts, and fails if the arena's oracle disagrees with BAN's `expected` or a scenario misses its oracle.

## Interoperability path

\`CLI -> protocol profile -> reference model -> event journal -> assertion/scorer\`

Future adapters can replace the reference model with a sandboxed Bisq regtest node over well-defined interfaces while preserving the scenario inputs. External integration will require license review and isolation of source-derived interfaces from the independent MIT simulator.
