"""Radio must start without display negotiation and survive an AA sender failure."""
import os,signal,subprocess,tempfile,time,unittest
from pathlib import Path
class SupervisorTests(unittest.TestCase):
 def test_wireless_independent_and_sender_restarts(self):
  root=Path(__file__).resolve().parents[1]
  tail=(root/'deploy/usb-test.sh').read_text().split('stage=session_android_auto',1)[1]
  with tempfile.TemporaryDirectory() as folder:
   p=Path(folder);(p/'bin').mkdir();(p/'bin/sleep').write_text('#!/bin/sh\n/bin/sleep 0.03\n');(p/'bin/sleep').chmod(0o755)
   (p/'catplay-aa-rx').write_text('#!/bin/sh\necho radio >> events\nexec /bin/sleep 30\n')
   (p/'aa-phone-v821').write_text('#!/bin/sh\necho aa >> events\nif [ ! -f failed ];then touch failed;exit 1;fi\nexec /bin/sleep 30\n')
   for n in ['catplay-aa-rx','aa-phone-v821']:(p/n).chmod(0o755)
   script=p/'run.sh';script.write_text('base="$PWD"\nSSID=test\nAA_VIDEO_SOCKET="$base/video.sock"\n'+tail)
   child=subprocess.Popen(['sh',str(script)],cwd=p,env={**os.environ,'PATH':str(p/'bin')+':'+os.environ['PATH']},start_new_session=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   try:
    end=time.monotonic()+3
    while time.monotonic()<end:
     events=(p/'events').read_text().splitlines() if (p/'events').exists() else []
     if events.count('aa')>=2:break
     time.sleep(.03)
    self.assertEqual(events.count('radio'),1)
    self.assertGreaterEqual(events.count('aa'),2)
    self.assertFalse((p/'video.sock.display').exists())
    self.assertIsNone(child.poll())
   finally:os.killpg(child.pid,signal.SIGTERM);child.wait()
