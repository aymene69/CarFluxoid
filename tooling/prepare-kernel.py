"""Apply vendor public-header/RISC-V patches needed to compile the gadget module.
The prebuilt beta kernel is retained, not replaced by a new kernel build.
"""
from pathlib import Path
import re,subprocess,shutil,json,hashlib
src=Path('/build/linux');recipe=Path('/build/firmware-sources/meta-sunxi/recipes-kernel/linux-sunxi')
for name in re.findall(r'file://([^;" ]+)',(recipe/'linux-sunxi_git.bb').read_text()):
 p=recipe/'files'/name
 if not name.endswith('.patch'):continue
 text=p.read_text()
 if not re.search(r'^diff --git a/(?:include/|arch/riscv/)',text,re.M):continue
 print('Kernel header patch:',name,flush=True)
 subprocess.run(['patch','--batch','--forward','-p1','-i',str(p)],cwd=src,check=True)
lock=json.loads(Path('/input/tooling/sources.lock.json').read_text())['profile']
for name,key in [('v821-module.config','module_config_sha256'),('beta-exports.symvers','exports_sha256')]:
 if hashlib.sha256((Path('/input/configs')/name).read_bytes()).hexdigest()!=lock[key]:raise RuntimeError('Kernel build input changed: '+name)
shutil.copyfile('/input/configs/v821-module.config',src/'.config')
