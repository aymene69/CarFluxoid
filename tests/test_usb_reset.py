import os,subprocess,tempfile,unittest
from pathlib import Path
SCRIPT=Path(__file__).resolve().parents[1]/'deploy/reset-usb-test.sh'
class ResetTests(unittest.TestCase):
 def test_module_removal_is_required(self):
  for failure in [True,False]:
   with self.subTest(removal_fails=failure),tempfile.TemporaryDirectory() as folder:
    root=Path(folder);module=root/'sys/module/aa_bulk';module.mkdir(parents=True)
    proc=root/'proc';proc.mkdir();(proc/'modules').write_text('aa_bulk 100 1\n')
    bind=root/'sys/class/iphone/test/bind';bind.parent.mkdir(parents=True);bind.write_text('0')
    soft=root/'sys/class/udc/test/soft_connect';soft.parent.mkdir(parents=True);soft.write_text('connect')
    bin=root/'bin';bin.mkdir()
    commands={'killall':'exit 0','sleep':'exit 0','rmmod':('exit 1' if failure else 'rmdir "$AA_RESET_SYS/module/aa_bulk"')}
    for n,body in commands.items():p=bin/n;p.write_text('#!/bin/sh\n'+body+'\n');p.chmod(0o755)
    env={**os.environ,'PATH':str(bin)+':'+os.environ['PATH'],'AA_RESET_SYS':str(root/'sys'),'AA_RESET_PROC':str(proc),'AA_RESET_LOCK':str(root/'lock')}
    p=subprocess.run(['sh',str(SCRIPT)],env=env,capture_output=True,text=True,timeout=5)
    self.assertEqual(p.returncode,1 if failure else 0,p.stdout+p.stderr)
    self.assertEqual(soft.read_text().strip(),'disconnect')
    self.assertEqual(bind.read_text().strip(),'0' if failure else '1')
    self.assertEqual('AA_RESET_OK' in p.stdout,not failure)
