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

- The `swarm` command uses a deterministic seed and synthetic offer book across many buyers/sellers. Keep fiat/BTC conservation across shared balances; prevent repeated matching of one offer ID. Test both `--policy reputation` and `--policy price`.
- The swarm's 2x-reputation preference is an experimental buyer heuristic, separate from actual Bisq offer-validation limits. Keep this distinction in results and documentation.
- Before merge, run `cargo run -- swarm --seed 7 --rounds 48` and a multi-seed test of conservation and reproducibility.
