# Agent engineering guidance

- Keep the initial Rust core dependency-free and deterministic.
- Treat all balances as integer satoshis. Preserve conservation and terminal-payout invariants.
- Whenever a receipt is added, test ACK-before-durable-commit and duplicate replay after recovery.
- Never reinterpret \`passed\` as \`safe\`; negative-control attacks intentionally produce findings.
- New fault injection points require deterministic regression tests.
- Keep Bitcoin keys, real money, real payment credentials, production network access, and untrusted RPC calls outside this simulation.
- When adding adapters, preserve the clean separation between a protocol model and the AISI/Redwood ControlArena integration.
- Run \`cargo fmt --all\`, \`cargo clippy --all-targets -- -D warnings\`, \`cargo test\`, and \`cargo run -- all\` before proposing a merge.
