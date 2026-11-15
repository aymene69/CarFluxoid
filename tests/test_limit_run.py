import subprocess,unittest
from pathlib import Path
SCRIPT=Path(__file__).resolve().parents[1]/'deploy/limit-run.sh'
class DeadlineTests(unittest.TestCase):
 def test_missing_command_fails(self):
  p=subprocess.run(['sh',str(SCRIPT),'5','/no-such-aa-command'],capture_output=True,text=True,timeout=8)
  self.assertEqual(p.returncode,127)
 def test_preserves_command_failure(self):
  p=subprocess.run(['sh',str(SCRIPT),'5','sh','-c','exit 7'],capture_output=True,text=True,timeout=8)
  self.assertEqual(p.returncode,7)
 def test_deadline_stops_child(self):
  p=subprocess.run(['sh',str(SCRIPT),'1','/bin/sleep','10'],capture_output=True,text=True,timeout=5)
  self.assertEqual(p.returncode,124)
  self.assertIn('AA_RAM_DEADLINE',p.stdout)
