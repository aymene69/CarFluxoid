"""Portable backup, verified composition and full-readback FEL flash/recovery."""
from pathlib import Path
import hashlib,json,gzip,struct,subprocess,os,re,time,socket,shlex
SIZE=0x1000000;START=0x440000;END=0xf40000
KERNELS={'14f8677b8c57a291e8d7045a43d110e4037cb113851dab226a3deb1cc6f47214','d893de2bdeab615533fdf60073a1a12b24b3fd56ca22c08e65ca56eb64bc5aff'}
def sha(b):return hashlib.sha256(b).hexdigest()
def private_dir(p):p.mkdir(mode=0o700,parents=True,exist_ok=True)
def write_private(path,data):
 path=Path(path);private_dir(path.parent)
 fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'wb') as f:f.write(data);f.flush();os.fsync(f.fileno())
def fit_kernel(image):
 f=image[0x40000:START]
 if len(f)<40 or struct.unpack_from('>I',f)[0]!=0xd00dfeed:raise ValueError('Expected beta FIT kernel partition')
 _,total,off,strings,*_=struct.unpack_from('>10I',f)
 if total>len(f) or off>=total or strings>=total:raise ValueError('Invalid FIT bounds')
 pos=off;stack=[]
 while pos+4<=total:
  token=struct.unpack_from('>I',f,pos)[0];pos+=4
  if token==1:
   end=f.index(b'\0',pos,total);stack.append(f[pos:end].decode());pos=(end+4)&~3
  elif token==2:stack.pop()
  elif token==3:
   length,name=struct.unpack_from('>II',f,pos);pos+=8
   if pos+length>total:raise ValueError('Invalid FIT property bounds')
   start=strings+name;end=f.index(b'\0',start,total);key=f[start:end].decode()
   if '/'.join(stack)=='/images/kernel-1' and key=='data':
    raw=f[pos:pos+length];return gzip.decompress(raw) if raw[:2]==b'\x1f\x8b' else raw
   pos=(pos+length+3)&~3
  elif token==4:continue
  elif token==9:break
  else:raise ValueError('Invalid FIT token')
 raise ValueError('Kernel payload missing')
def inspect(data):
 if len(data)!=SIZE:raise ValueError('Only the 16 MiB V821 RTL8733BS layout is supported')
 raw=fit_kernel(data);digest=sha(raw)
 if digest not in KERNELS:raise ValueError('Not the supported beta kernel; install the pinned base first')
 return {'size':len(data),'image_sha256':sha(data),'kernel_raw_sha256':digest,'kernel_partition_sha256':sha(data[0x40000:START]),'persist_sha256':sha(data[END:])}
def compose(backup,artifacts,output):
 before=Path(backup).read_bytes();reference=inspect(before);folder=Path(artifacts);proof=json.loads((folder/'manifest.json').read_text());root=(folder/'root.partition.bin').read_bytes()
 if len(root)!=END-START or sha(root)!=proof['root_sha256'] or root[1036]!=12:raise ValueError('Unverified rootfs partition')
 after=before[:START]+root+before[END:];write_private(Path(output),after)
 report={'schema':1,'before_sha256':sha(before),'after_sha256':sha(after),'kernel_bootloader_persist_preserved':True,'kernel':reference,'root_sha256':sha(root)}
 write_private(Path(str(output)+'.json'),(json.dumps(report,indent=2)+'\n').encode());return report
class Fel:
 def __init__(self,binary):self.binary=str(Path(binary).resolve())
 def run(self,*args,timeout=1800):return subprocess.run([self.binary,*map(str,args)],check=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=timeout).stdout
 def identify(self):
  info=self.run('version',timeout=10)
  if not ('1882' in info or 'V821' in info):raise ValueError('Not a V821 FEL device')
  chip=self.run('read32','0x4a000040',timeout=10)
  if not re.search(r'0x0*2\b',chip):raise ValueError('Not the tested V821 non-B revision: '+chip)
  flash=self.run('spinor',timeout=60)
  if not re.search(r"Found spi nor flash '[^']+' with 16777216 bytes",flash):raise ValueError('SPI geometry differs from supported 16 MiB target')
  print(info.strip());print(flash.strip())
 def read(self,path):
  path=Path(path)
  if path.exists():raise ValueError('Refusing to overwrite backup '+str(path))
  write_private(path,b'');self.run('spinor','read','0',hex(SIZE),path);path.chmod(0o600)
  data=path.read_bytes()
  if len(data)!=SIZE:raise ValueError('Incomplete SPI read')
  return data
 def backup(self,path):
  self.identify();path=Path(path);data=self.read(path);second=Path(str(path)+'.verification');other=self.read(second)
  if data!=other:raise ValueError('SPI reads differ; no write was performed')
  print('Two complete identical reads:',sha(data));return data
 def flash(self,image,backup_dir,restore=False,bootstrap=False,expected_before=None):
  image=Path(image);target=image.read_bytes()
  if len(target)!=SIZE:raise ValueError('Expected full 16 MiB image')
  folder=Path(backup_dir);private_dir(folder)
  before=self.backup(folder/'before.bin')
  if expected_before is not None and sha(before)!=expected_before:raise ValueError('Current device does not match supplied backup')
  if restore:
   # Full restoration is explicit; target must itself have two verified backup reads.
   paired=Path(str(image)+'.verification')
   if not paired.exists() or paired.read_bytes()!=target:raise ValueError('Restore requires the two identical original backup files')
   start,end=0,SIZE
  elif bootstrap:
   # Fresh device bootstrap writes the pinned public base, preserving its persist partition.
   if target[END:]!=before[END:]:raise ValueError('Bootstrap image must preserve this device persist bytes')
   inspect(target);start,end=0,END
  else:
   inspect(before);inspect(target)
   if before[:START]!=target[:START]:raise ValueError('Update would change boot/kernel; compose against the supported device backup')
   start,end=START,END
  expected=before[:start]+target[start:end]+before[end:]
  part=folder/'write-partition.bin';write_private(part,target[start:end])
  print(f'SPI write {start:#x}..{end:#x}; keep USB connected.',flush=True)
  self.run('spinor','write',hex(start),part)
  actual=self.read(folder/'after.bin')
  if actual!=expected:raise RuntimeError('Readback mismatch: keep USB connected and restore the verified backup')
  write_private(folder/'verification.json',(json.dumps({'image_sha256':sha(actual),'written_start':start,'written_end':end,'full_readback_verified':True},indent=2)+'\n').encode())
  print('Full 16 MiB readback matches. Replug the dongle.')
def ssh_connect(host):
 import paramiko
 sock=socket.create_connection((host,22),timeout=8);t=paramiko.Transport(sock);t.start_client(timeout=8);t.auth_none('root')
 if not t.is_authenticated():raise RuntimeError('SSH auth_none failed')
 t.set_keepalive(5);return t
def remote(t,cmd,timeout=60):
 c=t.open_session(timeout=8);c.settimeout(timeout);c.set_combine_stderr(True);c.exec_command(cmd);data=c.makefile('rb').read().decode(errors='replace');code=c.recv_exit_status();c.close()
 if code:raise RuntimeError(f'Remote command failed ({code}): {data}')
 return data
