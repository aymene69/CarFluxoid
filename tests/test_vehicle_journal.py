"""Bound RAM history and sparse durable checkpoints without touching identities."""
import os,subprocess,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
SCRIPT=ROOT/'deploy/vehicle-log.sh'
class JournalTests(unittest.TestCase):
 def run_boot(self,p,persist,times,extra=''):
  p.mkdir(parents=True,exist_ok=True);work=p/'work';work.mkdir(exist_ok=True)
  (work/'session.log').write_text('AUDIO_STATS healthy\n'*3000)
  (p/'uptime').write_text('0.00 0.00\n');(p/'timeline').write_text(''.join(f'{t}\n' for t in times))
  prefix='''
step=0
sleep(){ step=$((step+1)); value=$(sed -n "${step}p" "$TEST_DIR/timeline");[ -n "$value" ] || exit 0;printf '%s.00 0.00\\n' "$value" > "$TEST_DIR/uptime"; }
sync(){ echo sync >> "$TEST_DIR/sync-calls"; }
'''+extra
  env={**os.environ,'TEST_DIR':str(p),'AA_LOG_RAM_DIR':str(p/'ram'),'AA_LOG_PERSIST_DIR':str(persist),'AA_LOG_UPTIME_FILE':str(p/'uptime'),'AA_LOG_WORK_DIR':str(work)}
  result=subprocess.run(['sh','-c',prefix+'\n'+SCRIPT.read_text()],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,timeout=20)
  self.assertEqual(result.returncode,0,result.stderr.decode());return env
 def test_sparse_checkpoints_and_ram_bounds(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);persist=p/'persist';run=p/'run'
   self.run_boot(run,persist,[5,19,20,30,119,120,200,599,600,800,1799,1800,1900,3599,3600])
   self.assertEqual((run/'sync-calls').read_text().splitlines(),['sync']*10) # five checkpoints, not each snapshot
   self.assertEqual((run/'ram/last-save').read_text().strip(),'3600')
   reports=list(persist.glob('boot-*.log'));self.assertEqual(len(reports),1)
   for file in [reports[0],run/'ram/current.log']:
    self.assertLessEqual(file.stat().st_size,131072);self.assertIn('EARLY BOOT (RAM)',file.read_text())
   self.assertIn('AUDIO_STATS healthy',(run/'ram/current.log').read_text())
 def test_no_early_write_then_manual_save(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);persist=p/'persist';run=p/'run';env=self.run_boot(run,persist,[5,9])
   self.assertFalse(persist.exists());self.assertFalse((run/'sync-calls').exists())
   subprocess.run(['sh',str(SCRIPT),'--save'],env=env,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   self.assertEqual(len(list(persist.glob('boot-*.log'))),1)
 def test_three_boots_retained_and_restart_reuses_same_report(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);persist=p/'persist'
   for i in range(5):self.run_boot(p/f'boot{i}',persist,[20])
   reports=sorted(persist.glob('boot-*.log'));self.assertEqual(len(reports),3)
   self.assertEqual((persist/'current.log').resolve(),reports[-1].resolve())
   self.assertEqual((persist/'previous.log').resolve(),reports[-2].resolve())
   sequence=(persist/'boot-sequence').read_text()
   # Restart the collector within that same RAM boot: do not rotate saved boots.
   self.run_boot(p/'boot4',persist,[20,30])
   self.assertEqual((persist/'boot-sequence').read_text(),sequence)
 def test_low_space_preserves_old_reports_and_identity(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);persist=p/'persist';persist.mkdir();old=persist/'boot-00000001.log';old.write_text('old report')
   identity=p/'certificate.p7b';identity.write_bytes(b'private sentinel')
   fake='df(){ printf "Filesystem 1K-blocks Used Available Use%% Mounted\\n/dev/mtdblock3 768 760 8 99%% /persist\\n"; }\n'
   self.run_boot(p/'run',persist,[20,120,600],fake)
   self.assertEqual(old.read_text(),'old report');self.assertEqual(identity.read_bytes(),b'private sentinel')
   self.assertEqual(len(list(persist.glob('boot-*.log'))),1)
   self.assertFalse((persist/'boot-sequence').exists())
