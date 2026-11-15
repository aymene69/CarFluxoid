import os,subprocess,tempfile,unittest,time
from pathlib import Path
SCRIPT=Path(__file__).resolve().parents[1]/'deploy/clock-service.sh'
class ClockTests(unittest.TestCase):
 def test_restore_and_phone_update(self):
  with tempfile.TemporaryDirectory() as folder:
   root=Path(folder);bin=root/'bin';bin.mkdir();calls=root/'calls';saved=root/'epoch';source=root/'iphone'
   (bin/'date').write_text('#!/bin/sh\nif [ "$1" = +%s ]; then echo 31; else echo "$*" >> "$CALLS"; fi\n')
   (bin/'sync').write_text('#!/bin/sh\nexit 0\n')
   for p in bin.iterdir():p.chmod(0o755)
   env={**os.environ,'PATH':str(bin)+':'+os.environ['PATH'],'CALLS':str(calls),'AA_CLOCK_FILE':str(saved),'AA_CLOCK_SOURCE':str(source),'AA_CLOCK_BUILD':str(root/'build-epoch'),'AA_CLOCK_STATUS':str(root/'clock-status')}
   for invalid in ['','oops','123','999999999999999999999']:
    saved.write_text(invalid);subprocess.run(['sh',str(SCRIPT),'--restore'],env=env,check=True,capture_output=True)
   self.assertFalse(calls.exists())
   (root/'build-epoch').write_text('1794700000');subprocess.run(['sh',str(SCRIPT),'--restore'],env=env,check=True,capture_output=True)
   self.assertIn('source=build',(root/'clock-status').read_text())
   self.assertIn('@1794700000',calls.read_text())
   saved.write_text('1794730000');subprocess.run(['sh',str(SCRIPT),'--restore'],env=env,check=True,capture_output=True)
   self.assertIn('@1794730000',calls.read_text())
   source.write_text('1794810000');p=subprocess.Popen(['sh',str(SCRIPT)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
   try:
    deadline=time.monotonic()+3
    while saved.read_text().strip()!='1794810000' and time.monotonic()<deadline:time.sleep(.02)
    self.assertEqual(saved.read_text().strip(),'1794810000')
    self.assertIn('@1794810000',calls.read_text())
   finally:p.terminate();p.communicate(timeout=4)
