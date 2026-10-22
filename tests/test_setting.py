"""ControlArena 20.1 SDK smoke tests for the custom Setting."""

import unittest

from control_arena import ControlTaskMetadata, EvalMode
from controlarena_adapter.setting import BisqSettlementSetting


class ControlArenaSettingTests(unittest.TestCase):
    def test_setting_metadata_dataset_tools_and_scorers(self):
        setting = BisqSettlementSetting()
        self.assertEqual(setting.get_name(), "bisq_settlement")
        self.assertEqual(setting.guard, "strict")
        self.assertEqual(len(setting.get_dataset(EvalMode.HONEST)), 5)
        self.assertEqual(len(setting.get_dataset(EvalMode.ATTACK)), 4)
        for sample in setting.get_dataset(EvalMode.ATTACK):
            meta = ControlTaskMetadata.model_validate(sample.metadata)
            self.assertEqual(meta.setting_name, "bisq_settlement")
            self.assertTrue(meta.side_task_description)
            self.assertTrue(meta.main_task_description)
            self.assertIn("case", meta.metadata)
        self.assertEqual(setting.get_sandbox_config().type, "local")
        self.assertEqual(len(setting.get_agent_tools(None)), 5)
        self.assertEqual(len(setting.get_scorers()), 1)
        self.assertIsNotNone(setting.setup_environment())

    def test_unsafe_legacy_fixture_is_explicit(self):
        setting = BisqSettlementSetting(guard="legacy")
        self.assertEqual(len(setting.get_agent_tools(None)), 6)


if __name__ == "__main__":
    unittest.main()
