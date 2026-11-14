from pathlib import Path
import subprocess,json,hashlib,shutil,zipfile
from fetch import ROOT,fetch,sha
lock=fetch();base=ROOT/'local/base';base.mkdir(parents=True,exist_ok=True)
if not (base/'bundle/v821b-rtl8733bs/v821b-rtl8733bs.c2aflash').exists():
 archive=ROOT/'local/downloads'/lock['sources']['beta']['file']
 with zipfile.ZipFile(archive) as z:z.extractall(base/'zip')
 nested=list((base/'zip').rglob('*.7z'));assert len(nested)==1
 subprocess.run([shutil.which('7zz') or shutil.which('7z') or '7zr','x','-y',f'-o{base}/bundle',str(nested[0])],check=True,stdout=subprocess.DEVNULL)
image=base/'bundle/v821b-rtl8733bs/v821b-rtl8733bs.c2aflash';data=image.read_bytes();assert len(data)==0x1000000
if sha(image)!=lock['profile']['beta_image_sha256']:raise RuntimeError('Unpacked base image checksum mismatch')
root=Path('/build/firmware-root')
marker=Path('/build/firmware-root.sha256')
if root.exists() and (not marker.exists() or marker.read_text()!=sha(image)):shutil.rmtree(root)
if not root.exists():
 erofs=base/'beta-root.partition';erofs.write_bytes(data[0x440000:0xf40000]);subprocess.run(['fsck.erofs','--extract='+str(root),'--preserve-perms','--no-preserve-owner',str(erofs)],check=True)
marker.write_text(sha(image))
(base/'reference.json').write_text(json.dumps({'image_sha256':sha(image),'kernel_sha256':hashlib.sha256(data[0x40000:0x440000]).hexdigest(),'boot0_sha256':hashlib.sha256(data[:0x40000]).hexdigest(),'root_sha256':hashlib.sha256(data[0x440000:0xf40000]).hexdigest()},indent=2)+'\n')
print('Pinned beta and runtime sysroot ready.')
