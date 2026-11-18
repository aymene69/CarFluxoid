"""Persist separate boots without writing diagnostics during early hardware init."""
import subprocess,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
class JournalTests(unittest.TestCase):
 def test_retention_bounds_and_deferred_persistence(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);directory=p/'persist';noise=p/'noise';noise.write_text('log-record\n'*4000)
   source=(ROOT/'deploy/vehicle-log.sh').read_text().replace('dir=/persist/aa-diagnostics','dir='+str(directory)).replace('early=/tmp/aa-early-boot-$$.log','early='+str(p/'early.log')).replace('tmp=/tmp/aa-vehicle-snapshot-$$','tmp='+str(p/'snapshot'))
   for f in ['/var/log/aa-bridge-boot.log','/tmp/aa-autoboot/phone.log','/tmp/aa-autoboot/carplay.log','/var/log/bluetooth.log']:source=source.replace(f,str(noise))
   # The initial sleep must precede any new boot-sequence/report writes.
   stub='sleep(){ if [ "$1" = 8 ];then [ ! -f "'+str(directory)+'/boot-sequence.new" ] || exit 99;else ticks=$(( ${ticks:-0}+1 ));[ "$ticks" -lt 6 ] || exit 0;fi; }\n'
   script=p/'run.sh';script.write_text('#!/bin/sh\n'+stub+source)
   for _ in range(5):subprocess.run(['sh',str(script)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True,timeout=15)
   logs=sorted(directory.glob('boot-*.log'));self.assertEqual(len(logs),3)
   self.assertEqual((directory/'current.log').resolve(),logs[-1].resolve())
   self.assertEqual((directory/'previous.log').resolve(),logs[-2].resolve())
   for log in logs:
    self.assertLessEqual(log.stat().st_size,131072)
    self.assertIn('EARLY BOOT (RAM)',log.read_text())
