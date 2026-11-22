"""Unit tests for the Raspberry Pi projected touchscreen dashboard."""
import struct
import sys
import tempfile
import types
import unittest
from pathlib import Path

if "PIL" not in sys.modules:
    pil = types.ModuleType("PIL")
    for name in ("Image", "ImageDraw", "ImageFont", "ImageOps"):
        setattr(pil, name, types.SimpleNamespace())
    sys.modules["PIL"] = pil

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "platforms" / "raspberrypi" / "dashboard"))
from dashboard import Dashboard, WAIT_BACK  # noqa: E402


class DashboardTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.settings = root / "settings"
        self.assets = root / "assets"
        self.runtime = root / "runtime"
        self.bridge = root / "bridge"
        for d in (self.settings, self.assets, self.runtime, self.bridge):
            d.mkdir()
        self.ui = Dashboard(self.settings, self.assets, self.runtime, self.bridge)
        self.started = []
        self.actions = []
        self.ui.start_carplay = lambda restart=False: self.started.append(restart)
        self.ui.shell_action = lambda code: self.actions.append(code)

    def tearDown(self):
        self.ui.control.close()
        self.tmp.cleanup()

    def test_auto_start_settles_two_seconds_and_skips_redundant_publish(self):
        (self.bridge / "video.sock.display").write_text("800 480 0 32\n")
        (self.bridge / "video.sock.control").write_text("")
        (self.runtime / "dashboard-wait-800x480.h264").write_bytes(b"idr")

        self.assertFalse(self.ui.maybe_auto_start())
        self.assertEqual(self.started, [])
        self.assertIsNotNone(self.ui.auto_start_pending)

        # Still under 2 s -> must not fire yet.
        self.assertFalse(self.ui.maybe_auto_start())
        self.assertEqual(self.started, [])

        # Past 2 s -> starts CarPlay without forcing ffmpeg re-encode.
        key, t0 = self.ui.auto_start_pending
        self.ui.auto_start_pending = (key, t0 - 2.5)
        self.assertFalse(self.ui.maybe_auto_start())
        self.assertEqual(self.started, [False])

        # Same head-unit session does not re-trigger.
        self.assertFalse(self.ui.maybe_auto_start())
        self.assertEqual(self.started, [False])

    def test_user_interaction_cancels_pending_auto_start(self):
        (self.bridge / "video.sock.display").write_text("800 480 0 32\n")
        (self.bridge / "video.sock.control").write_text("")
        self.assertFalse(self.ui.maybe_auto_start())
        self.assertIsNotNone(self.ui.auto_start_pending)

        self.ui.choose("settings")
        self.assertEqual(self.ui.page, "settings")
        self.assertIsNone(self.ui.auto_start_pending)
        self.assertFalse(self.ui.maybe_auto_start())
        self.assertEqual(self.started, [])

    def test_auto_start_can_be_disabled_via_options(self):
        (self.settings / "options.txt").write_text("auto_start=false\n")
        (self.bridge / "video.sock.display").write_text("800 480 0 32\n")
        (self.bridge / "video.sock.control").write_text("")
        self.assertFalse(self.ui.maybe_auto_start())
        self.assertIsNone(self.ui.auto_start_pending)

    def test_settings_cycle_persists_options(self):
        self.ui.choose("auto_start")
        self.assertEqual(self.ui.option("auto_start", "true"), "false")
        self.ui.choose("theme")
        self.assertEqual(self.ui.option("theme", "auto"), "day")
        self.ui.choose("fps")
        self.assertEqual(self.ui.option("fps", "30"), "20")

    def test_waiting_back_button_touch_returns_to_home(self):
        x = (WAIT_BACK[0] + WAIT_BACK[2]) // 2
        y = (WAIT_BACK[1] + WAIT_BACK[3]) // 2
        down = struct.pack("<BHH", 0x80 | 0, x, y)
        up = struct.pack("<BHH", 0x80 | 1, x, y)
        self.assertFalse(self.ui.touch(down))
        self.assertTrue(self.ui.touch(up))
        self.assertEqual(self.ui.page, "home")
        self.assertEqual(self.actions, [8])

    def test_radio_name_validation(self):
        self.ui.choose("wifi")
        self.ui.draft = "   "
        self.ui.choose("save")
        self.assertEqual(self.ui.page, "wifi")
        self.assertIn("invalide", self.ui.notice)

        self.ui.draft = "MyCar-AP"
        self.ui.choose("save")
        self.assertEqual(self.ui.page, "network")
        self.assertEqual(self.ui.radio_name("wifi"), "MyCar-AP")


if __name__ == "__main__":
    unittest.main()
