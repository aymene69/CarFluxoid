"""SSH backups must tolerate stale remote files and retain two matching reads."""
import io,sys,tempfile,types,unittest
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import carfluxoid
class BackupTests(unittest.TestCase):
 def test_unique_remote_names_and_cleanup_after_local_save(self):
  commands=[];opened=[];removed=[]
  class Transport:
   def close(self):pass
  class Sftp:
   def __enter__(self):return self
   def __exit__(self,*args):pass
   def open(self,name,mode):opened.append(name);return io.BytesIO(b'backup')
   def remove(self,name):removed.append(name)
  def remote(t,command,*args):
   commands.append(command)
   return 'riscv32 v821b-rtl8733bs' if command.startswith('uname') else ''
  fake=types.SimpleNamespace(SFTPClient=types.SimpleNamespace(from_transport=lambda t:Sftp()))
  with tempfile.TemporaryDirectory() as directory,patch.dict(sys.modules,paramiko=fake),patch.object(carfluxoid,'ssh_connect',return_value=Transport()),patch.object(carfluxoid,'remote',side_effect=remote),patch.object(carfluxoid,'SIZE',6),patch.object(carfluxoid,'inspect'):
   for n in range(2):
    path=Path(directory)/str(n)
    self.assertEqual(carfluxoid.backup_ssh('host',path),b'backup')
    self.assertEqual(path.read_bytes(),Path(str(path)+'.verification').read_bytes())
  self.assertNotEqual(opened[0],opened[2])
  self.assertEqual(opened,[removed[0],removed[0],removed[1],removed[1]])
  self.assertTrue(all('/tmp/carfluxoid-backup-' in p for p in removed))
