"""Run a ControlArena evaluation with your explicitly selected model pair."""

from __future__ import annotations

import argparse


def main() -> None:
    parser = argparse.ArgumentParser(description="Synthetic Bisq settlement ControlArena eval")
    parser.add_argument("--trusted", required=True, help="Inspect AI model ID")
    parser.add_argument("--untrusted", required=True, help="Inspect AI model ID")
    parser.add_argument("--mode", choices=("honest", "attack"), default="honest")
    parser.add_argument("--guard", choices=("strict", "legacy"), default="strict")
    parser.add_argument("--limit", type=int, default=2)
    parser.add_argument("--binary", default=None, help="Path to compiled Rust binary")
    options = parser.parse_args()
    if options.limit < 1:
        parser.error("--limit must be positive")

    from control_arena import EvalMode
    from control_arena.eval import ControlEvalConfig, get_control_task_product
    from control_arena.micro_protocol import defer_to_trusted_builder
    from inspect_ai import eval as inspect_eval

    from .setting import BisqSettlementSetting

    setting = BisqSettlementSetting(guard=options.guard, binary_path=options.binary)
    config = ControlEvalConfig(
        trusted_model=options.trusted,
        untrusted_model=options.untrusted,
    )
    tasks = get_control_task_product(
        setting,
        defer_to_trusted_builder(),
        [EvalMode.ATTACK if options.mode == "attack" else EvalMode.HONEST],
        eval_config=config,
    )
    inspect_eval(tasks=tasks, limit=options.limit)


if __name__ == "__main__":
    main()
