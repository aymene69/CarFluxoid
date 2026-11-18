#!/usr/bin/env python3
"""CarFluxoid build/install entry point. Hardware writes occur only in flash/restore/base-install commands."""
from pathlib import Path
import argparse,subprocess,sys,json,os,shutil,tarfile,urllib.request,hashlib,time,uuid
ROOT=Path(__file__).resolve().parent;sys.path.insert(0,str(ROOT/'tooling'))
from fetch import fetch,sha
from device import Fel,compose,inspect,private_dir,write_private,ssh_connect,remote,SIZE,START,END
IMAGE='carfluxoid-sdk:2026-11-14'
def cmd(args,cwd=None):subprocess.run(list(map(str,args)),cwd=cwd,check=True)
def xfel():
 p=ROOT/'local/xfel/xfel'
 if not p.exists():raise RuntimeError('Run: python3 carfluxoid.py xfel-build')
 return p
def tls():
 # This is a published reference identity, not a device-specific MFi certificate.
 folder=ROOT/'local/tls';private_dir(folder)
 base='https://raw.githubusercontent.com/iscle/android-auto-certificates/8dc25fe2bd2c57e0c6f48488bb1095ecc0c11e31/android-auto/latest/'
 for name,digest in [('cert','43c05285265d83a9e1ae74da7359f07c081c711354a2c397292419169d8c360f'),('key','e30eade4b8f3384b739400a988be494d639e92264e8f60414f17c4c931f333e2')]:
  p=folder/f'test-{name}.pem'
  if p.exists():continue
  with urllib.request.urlopen(base+('certificate.pem' if name=='cert' else 'private-key.pem'),timeout=60) as r:data=r.read()
  if hashlib.sha256(data).hexdigest()!=digest:raise RuntimeError('Reference TLS checksum mismatch')
  write_private(p,data)
 cert=subprocess.check_output(['openssl','x509','-in',str(folder/'test-cert.pem'),'-pubkey','-noout'])
 key=subprocess.check_output(['openssl','pkey','-in',str(folder/'test-key.pem'),'-pubout'])
 if cert!=key:raise RuntimeError('TLS key does not match certificate')
 cmd(['openssl','x509','-checkend','0','-noout','-in',folder/'test-cert.pem'])
 print('Local reference TLS identity ready. Compatibility and expiry are receiver-dependent.')
def backup_ssh(host,path):
 path=Path(path);t=ssh_connect(host)
 temporary="/tmp/carfluxoid-backup-"+uuid.uuid4().hex+".bin"
 try:
  identity=remote(t,'uname -a; cat /proc/mtd; cat /proc/cmdline')
  if 'riscv32' not in identity or 'v821b-rtl8733bs' not in identity or 'root=/dev/initrd' in identity:raise RuntimeError('Expected normal V821 RTL8733BS beta, not recovery/other hardware')
  remote(t,'carlinkit_otalib backup '+temporary,120)
  import paramiko
  with paramiko.SFTPClient.from_transport(t) as s:
   with s.open(temporary,'rb') as f:first=f.read()
   with s.open(temporary,'rb') as f:second=f.read()
  if len(first)!=SIZE or first!=second:raise RuntimeError('Backup size/read mismatch')
  inspect(first);write_private(path,first);write_private(Path(str(path)+'.verification'),second)
  with paramiko.SFTPClient.from_transport(t) as s:s.remove(temporary)
  print('Two complete matching backup reads:',hashlib.sha256(first).hexdigest());return first
 finally:t.close()
def flash_ssh(host,image,out):
 image=Path(image);target=image.read_bytes();inspect(target);folder=Path(out);private_dir(folder)
 # Stop the independent supervisor before stopping both bridge executables.
 t=ssh_connect(host)
 try:
  identity=remote(t,'uname -a; cat /proc/mtd; cat /proc/cmdline')
  if 'riscv32' not in identity or 'v821b-rtl8733bs' not in identity or 'root=/dev/initrd' in identity:raise RuntimeError('Wrong hardware or recovery mode; no services stopped')
  remote(t,'if [ -f /tmp/aa-web-supervisor.lock/pid ]; then kill "$(cat /tmp/aa-web-supervisor.lock/pid)" 2>/dev/null || true; fi; killall aa-phone-v821 catplay-aa-rx 2>/dev/null || true')
 finally:t.close()
 before=backup_ssh(host,folder/'before.bin')
 if before[:START]!=target[:START]:raise RuntimeError('Image boot/kernel does not match this device')
 # Persist can legitimately change between compose and flash. Always keep the fresh bytes.
 target=before[:START]+target[START:END]+before[END:]
 image=folder/'rebased.c2aflash';write_private(image,target)
 t=ssh_connect(host)
 try:
  import paramiko
  with paramiko.SFTPClient.from_transport(t) as s:s.put(str(image),'/tmp/carfluxoid-fw.bin')
  uploaded=remote(t,'sha256sum /tmp/carfluxoid-fw.bin').split()[0]
  if uploaded!=hashlib.sha256(target).hexdigest():raise RuntimeError('Upload checksum mismatch')
  print('Flashing verified image. Keep USB powered.',flush=True)
  print(remote(t,"sync; mount -o remount,ro /persist || exit $?; carlinkit_otalib flash /tmp/carfluxoid-fw.bin || exit $?; nohup sh -c 'sleep 2; reboot' >/dev/null 2>&1 </dev/null &",120))
 finally:t.close()
 # Read back after normal boot; no success claim until the flashed partitions match.
 time.sleep(8);last=None
 for _ in range(30):
  try:
   actual=backup_ssh(host,folder/'after.bin')
   if actual[:END]!=target[:END]:raise RuntimeError('Flashed boot/kernel/rootfs readback mismatch')
   print('Normal boot and flash readback verified. Persist may change normally at boot.');return
  except (OSError,EOFError) as e:last=e;time.sleep(3)
 raise RuntimeError('SSH readback not available; verify Wi-Fi and use FEL recovery. '+str(last))
def main():
 p=argparse.ArgumentParser(description=__doc__);sub=p.add_subparsers(dest='action',required=True)
 for name in ['fetch','tls','sdk','build','test','xfel-build','stock-to-fel']:sub.add_parser(name)
 sub.choices['stock-to-fel'].add_argument('--preset',choices=['carlinkit','wooboobox','ekiy'],required=True)
 for name in ['inspect']:
  q=sub.add_parser(name);q.add_argument('image',type=Path)
 q=sub.add_parser('compose');q.add_argument('--backup',required=True,type=Path);q.add_argument('--output',required=True,type=Path)
 for name in ['backup','flash']:
  q=sub.add_parser(name);q.add_argument('--host',default='192.168.50.2');q.add_argument('--output',type=Path,required=True)
  if name=='flash':q.add_argument('--image',type=Path,required=True)
 for name in ['fel-backup','fel-flash','restore','base-install']:
  q=sub.add_parser(name);q.add_argument('--output',required=True,type=Path)
  if name in ['fel-flash','restore']:q.add_argument('--image',required=True,type=Path)
  if name=='base-install':q.add_argument('--stock-backup',required=True,type=Path)
 args=p.parse_args();a=args.action
 if a=='fetch':fetch()
 elif a=='tls':tls()
 elif a=='sdk':
  env=os.environ.copy();env['BUILDX_CONFIG']=str(ROOT/'local/buildx')
  subprocess.run(['docker','build','-t',IMAGE,'-f',str(ROOT/'tooling/Dockerfile'),str(ROOT/'tooling')],check=True,env=env)
 elif a=='build':
  fetch();tls();cmd([ROOT/'scripts/prepare-catplay.sh'])
  cmd(['docker','run','--rm','-e','JOBS=4','-v',str(ROOT)+':/input','-v','carfluxoid-sdk:/build',IMAGE,'/input/tooling/build.sh'])
 elif a=='test':cmd(['docker','run','--rm','-v',str(ROOT)+':/input','-v','carfluxoid-sdk:/build',IMAGE,'python3','/input/tooling/test.py'])
 elif a=='xfel-build':
  fetch();dest=ROOT/'local/xfel'
  if not dest.exists():
   dest.mkdir();cmd(['tar','-xf',ROOT/'local/downloads/xfel.tar.gz','--strip-components=1','-C',dest])
  cmd(['make','-j4'],cwd=dest)
 elif a=='stock-to-fel':
  if sys.version_info<(3,12):raise RuntimeError('The upstream stock bootstrap requires host Python 3.12 or newer')
  source=ROOT/'local/base/bundle/v821b-rtl8733bs/tools'
  if not source.exists():raise RuntimeError('Run build first to unpack the pinned base and its vendor bootstrap tools')
  cmd([sys.executable,source/'cpbox2fel.py',args.preset],cwd=source)
 elif a=='inspect':print(json.dumps(inspect(args.image.read_bytes()),indent=2))
 elif a=='compose':print(json.dumps(compose(args.backup,ROOT/'local/artifacts',args.output),indent=2))
 elif a=='backup':backup_ssh(args.host,args.output)
 elif a=='flash':flash_ssh(args.host,args.image,args.output)
 elif a=='fel-backup':Fel(xfel()).backup(args.output)
 elif a=='fel-flash':Fel(xfel()).flash(args.image,args.output)
 elif a=='restore':Fel(xfel()).flash(args.image,args.output,restore=True)
 elif a=='base-install':
  before=args.stock_backup.read_bytes();paired=Path(str(args.stock_backup)+'.verification')
  if len(before)!=SIZE or not paired.exists() or paired.read_bytes()!=before:raise RuntimeError('First take two verified stock FEL backup reads')
  base=ROOT/'local/base/bundle/v821b-rtl8733bs/v821b-rtl8733bs.c2aflash';raw=base.read_bytes();proof=json.loads((ROOT/'local/base/reference.json').read_text())
  if hashlib.sha256(raw).hexdigest()!=proof['image_sha256']:raise RuntimeError('Pinned base changed')
  private_dir(args.output);target=args.output/'base-with-own-persist.c2aflash';write_private(target,raw[:END]+before[END:]);Fel(xfel()).flash(target,args.output/'readback',bootstrap=True,expected_before=hashlib.sha256(before).hexdigest())
if __name__=='__main__':
 try:main()
 except (Exception,KeyboardInterrupt) as e:print('STOP:',e,file=sys.stderr);sys.exit(1)
