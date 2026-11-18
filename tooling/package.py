"""Build an identity-free rootfs partition; compose it with each user's own backup later."""
from pathlib import Path
import shutil,gzip,subprocess,json,hashlib,os,urllib.request,time
from fetch import ROOT,sha
out=ROOT/'local/artifacts';root=Path('/build/carfluxoid-root');check=Path('/build/carfluxoid-verified')
for p in [root,check]:
 if p.exists():shutil.rmtree(p)
shutil.copytree('/build/firmware-root',root,symlinks=True)
dest=root/'usr/lib/aa-bridge';dest.mkdir(exist_ok=True,parents=True)
for n in ['aa-phone-v821','catplay-aa-rx','aa_bulk.ko']:
 data=(out/n).read_bytes()
 if data[:4]!=b'\x7fELF' or data[4]!=1 or int.from_bytes(data[18:20],'little')!=243:raise RuntimeError(n+': expected RV32 ELF')
 (dest/(n+'.gz')).write_bytes(gzip.compress(data,9,mtime=0))
for n in ['boot.sh','usb-test.sh','web-service.sh','log-maintenance.sh','clock-service.sh','vehicle-log.sh','radio-service.sh']:shutil.copy2(ROOT/'deploy'/n,dest/n)
profile=os.environ.get('CARFLUXOID_USB_PROFILE')
if profile:
 if profile not in ['legacy','direct','negotiated']:raise RuntimeError('Invalid USB profile')
 (dest/'usb-profile').write_text(profile+'\n')
(dest/'build-epoch').write_text(str(int(os.environ.get('SOURCE_DATE_EPOCH',time.time())))+'\n')
shutil.copy2(ROOT/'deploy/wifi-env.sh.example',dest/'wifi-env.sh')
shutil.copy2(ROOT/'assets/mire.h264',dest/'mire.h264')
for p in (ROOT/'assets/waiting').glob('*.h264'):shutil.copy2(p,dest/p.name)
app=root/'usr/bin/catplay_c2a';shutil.copy2(ROOT/'deploy/catplay_c2a',app);app.chmod(0o755)
# The selected standard developer TLS identity is fetched from a pinned public reference,
# not from the owner's MFi identity or pairing database.
tls=ROOT/'local/tls';tls.mkdir(mode=0o700,exist_ok=True)
for n in ['cert','key']:
 target=tls/f'test-{n}.pem'
 if not target.exists():raise RuntimeError('Missing local TLS identity; run: python3 carfluxoid.py tls')
 shutil.copy2(target,dest/target.name)
(dest/'test-key.pem').chmod(0o600)
erofs=out/'root.erofs';subprocess.run(['mkfs.erofs','-b4096','-zlz4hc,level=9','-E','dedupe,fragments,ztailpacking','--all-root',str(erofs),str(root)],check=True,stdout=subprocess.DEVNULL)
data=erofs.read_bytes()
if data[1036]!=12 or len(data)>0xb00000:raise RuntimeError('Rootfs must fit 11 MiB with 4 KiB blocks')
subprocess.run(['fsck.erofs','--extract='+str(check),'--preserve-perms','--no-preserve-owner',str(erofs)],check=True)
def tree(path):
 import stat
 result={}
 for parent,dirs,files in os.walk(path,followlinks=False):
  for n in dirs+files:
   p=Path(parent)/n;s=p.lstat();v=[stat.S_IFMT(s.st_mode),stat.S_IMODE(s.st_mode)]
   if p.is_symlink():v.append(os.readlink(p))
   elif p.is_file():v.append(sha(p))
   result[str(p.relative_to(path))]=v
 return result
if tree(root)!=tree(check):raise RuntimeError('EROFS extraction differs from staged root')
# Only the launcher is replaced; all other original beta files are retained.
before=tree(Path('/build/firmware-root'));after=tree(check)
changes=[n for n in before if before[n]!=after.get(n)]
if changes!=['usr/bin/catplay_c2a']:raise RuntimeError('Unexpected base modification: '+str(changes))
partition=data+b'\xff'*(0xb00000-len(data));(out/'root.partition.bin').write_bytes(partition)
manifest={'schema':1,'root_sha256':hashlib.sha256(partition).hexdigest(),'rootfs_bytes':len(data),'erofs_block_size':4096,'base':json.loads((ROOT/'local/base/reference.json').read_text()),'payload':{n:sha(out/n) for n in ['aa-phone-v821','catplay-aa-rx','aa_bulk.ko']},'changed_base_files':changes,'usb_profile':profile or 'user-setting','module_validation':json.loads((ROOT/'tooling/sources.lock.json').read_text())['profile'].get('module_validation','hardware-tested reference')}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Verified rootfs built:',out/'root.partition.bin')
