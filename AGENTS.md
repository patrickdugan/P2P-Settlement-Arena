# Coding agent instructions

- **Bisq Easy is the default market profile**. It is payment-first, reputation-backed, and has no escrow. Use \`cargo run --\` and \`cargo run -- all\` to exercise it.
- The secondary \`bisq-musig\` profile is an abstract escrow/crash/watchtower research model. Use \`cargo run -- simulate --profile bisq-musig all\`. Never conflate it with Bisq Easy or claim real MuSig cryptography is implemented.
- Consult \`docs/PROTOCOL_PROFILES.md\` and current Bisq documentation before changing trade rules, reputation calculations, fee modeling, or price oracle checks.
- Keep production dependencies at zero for now and tests deterministic. Use integer fiat cents and integer satoshis, with checked arithmetic and independent conservation properties.
- Maintain a strict distinction between a buyer's statement that they paid and an external modeled bank transfer. BTC must not be sent based on the statement alone.
- Any new attack path needs a legitimate baseline, a defended or intentionally vulnerable outcome, a reproducible oracle, and an automated regression test.
- \`passed=true\` means the benchmark oracle behaved as designed, while \`attack_success=true\` means a forbidden outcome occurred in the simulated model.
- Do not use real keys, wallets, production endpoints, user data, or fiat credentials in the arena.
- Do not copy AGPL-3.0 Bisq 2 sources into this MIT codebase. Link to upstream specifications and implement behavior independently.
- Run \`cargo fmt --all -- --check\`, \`cargo clippy --all-targets -- -D warnings\`, \`cargo test --all-targets\`, \`cargo run -- all\`, and \`cargo run -- simulate --profile bisq-musig all\`.

- The cashout profiles (`cashout-btc-first`, `cashout-bisq-easy`) implement the BAN contract `ban-arena-cashout/1`. The provider BUYS BTC, so it is always the arena's buyer, never its seller; the recipient is a third-party `fiat_payee`. Keep the two profiles separate: the exposure falls on the sender in one and on the provider in the other.
- In both cashout profiles the recipient is credited only by a modeled rail event, never by the provider's claim, and the sender's BTC counts only as an output in the provider's own chain view. Result lines carry exactly the key set BAN's `parse_arena_line` accepts for the cashout protocols.
- `fixtures/ban/` is a byte-for-byte copy of BAN's scenarios; refresh it only from a new BAN commit. Run `cargo run -- simulate --profile cashout-btc-first all`, `cargo run -- simulate --profile cashout-bisq-easy all` and `cargo run -- cashout-import fixtures/ban/cashout_scenarios.jsonl`.

- The `swarm` command uses a deterministic seed and synthetic offer book across many buyers/sellers. Keep fiat/BTC conservation across shared balances; prevent repeated matching of one offer ID. Test both `--policy reputation` and `--policy price`.
- The swarm's 2x-reputation preference is an experimental buyer heuristic, separate from actual Bisq offer-validation limits. Keep this distinction in results and documentation.
- Before merge, run `cargo run -- swarm --seed 7 --rounds 48` and a multi-seed test of conservation and reproducibility.
- Use `cargo run -- swarm-compare --seed N --rounds R --trace` for paired policy experiments. Preserve policy-independent, counter-keyed buyer arrivals and seller quote proposals.

- The custom ControlArena Setting lives under controlarena_adapter/ and runs the stateless Rust control gateway. Only the scenario scheduler inserts genuine synthetic bank events; never expose an agent tool for arbitrary bank-credit events.
- Keep the strict guard as default; the legacy trust-claim guard is a deliberately vulnerable negative control for alignment evaluation.
- Test both Rust fixture outputs, Python stdlib gateway tests, and the optional ControlArena SDK Setting smoke suite. Preserve per-sample Inspect StoreModel action histories and avoid process-global mutable state.


- New Bitcoin Core driver is strictly regtest-only, and external actions are opt-in. Require chain name, canonical genesis hash, and 127.0.0.1 RPC before ANY wallet changes, mining, broadcast, or invalidation. Never reuse production credentials or wallets.
- Bisq REST observer is GET-only, loopback-bound and suppresses raw trade data. No agent-exposed mutation endpoints.
- Keep Bitcoin Core regtest payout distinct from a real Bisq trade: fiat receipt is synthetic. The bitcoin/bitcoin Docker image used in CI is unofficial and for disposable tests only.


- The two-node Bisq launcher must require --launch before starting upstream Java, maintain separate 0700 data directories, and keep local API session handling enabled. App profiles are isolated; external network egress is NOT necessarily blocked by local CLEAR-only P2P configuration.
- Pairing QR and client session data are credentials. Never print, upload, or commit them. Session export is opt-in with private 0600 files in an ephemeral workspace.
- Two-node trade subscriptions are read-only. A regtest TXID may be marked correlated only with authenticated state and amount observations from BOTH nodes and a Bitcoin Core regtest transaction that actually pays the observed bcrt1 address.
- Do not conflate a signed Bisq trade, actual fiat receipt, and Bitcoin Core transaction; the current adapter observes/links, while fiat remains synthetic and Bitcoin sends occur only in the separate opt-in regtest driver.
