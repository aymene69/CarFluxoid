import os,subprocess,tempfile,unittest
from pathlib import Path
SCRIPT=Path(__file__).resolve().parents[1]/'scripts/disable-vendor-persistent-logs.sh'
class VendorLogging(unittest.TestCase):
 def test_preserves_settings_and_is_idempotent(self):
  with tempfile.TemporaryDirectory() as td:
   path=Path(td)/'config.txt';original='wifi_name=CarFluxoid\nlog_persist=1\nvideo=keep\nlog_persist=1\n'
   path.write_text(original);env={**os.environ,'AA_VENDOR_CONFIG':str(path)}
   subprocess.run(['sh',str(SCRIPT)],check=True,env=env,capture_output=True)
   self.assertEqual(path.read_text(),'wifi_name=CarFluxoid\nlog_persist=0\nvideo=keep\n')
   self.assertEqual(Path(str(path)+'.before-lowwrite').read_text(),original)
   timestamp=path.stat().st_mtime_ns
   subprocess.run(['sh',str(SCRIPT)],check=True,env=env,capture_output=True)
   self.assertEqual(path.stat().st_mtime_ns,timestamp)
