"""Run native protocol tests plus the production RV32 settings and recorder tests."""
from pathlib import Path
import subprocess,os,json,shutil,tempfile
root=Path('/input');build=Path('/build/aa');host=build/'mbedtls-host'
if not host.exists():shutil.copytree(build/'mbedtls-rv',host,ignore=shutil.ignore_patterns('*.o','*.a','*.so'))
subprocess.run(['make','-C',str(host),'-j4','lib'],check=True,stdout=subprocess.DEVNULL)
for name in ['parser','microphone','video_config','aoa','reader_eof','live_restart']:
 binary=build/('test-'+name)
 subprocess.run(['cc','-DSYS_io_pgetevents_time64=SYS_io_pgetevents','-I'+str(host/'include'),str(root/'bridge-aa/tests'/f'{name}.c'),*[str(host/'library'/('lib'+n+'.a')) for n in ['mbedtls','mbedx509','mbedcrypto']],'-lpthread','-o',str(binary)],check=True)
 subprocess.run([str(binary)],check=True)
env={**os.environ,'AA_WEB_TEST_COMMAND':'qemu-riscv32 -L /build/firmware-root /input/local/artifacts/catplay-aa-rx --web'}
subprocess.run(['python3','/input/bridge-aa/tests/test_web.py'],check=True,env=env)
script=(root/'bridge-aa/build-carplay-rx.sh').read_text().replace('cargo" build --release','cargo" test --no-run --message-format=json --release').replace('-C panic=abort','-C panic=abort -Zpanic_abort_tests')
p=build/'test-rx.sh';p.write_text(script)
env={**os.environ,'CARFLUXOID_SYSROOT':'/build/firmware-root'}
r=subprocess.run(['sh',str(p)],check=True,stdout=subprocess.PIPE,text=True,env=env)
executables=[]
for line in r.stdout.splitlines():
 try:data=json.loads(line)
 except ValueError:continue
 if data.get('reason')=='compiler-artifact' and data.get('profile',{}).get('test') and data.get('executable') and data.get('target',{}).get('name')=='catplay-aa-rx':executables.append(data['executable'])
if not executables:raise RuntimeError('No RV32 test executable found')
for executable in executables:subprocess.run(['qemu-riscv32','-L','/build/firmware-root',executable,'-Z','unstable-options','--force-run-in-process','--test-threads=1'],check=True,env={**env,'RUSTC_BOOTSTRAP':'1'})
subprocess.run(['python3','-m','unittest','discover','-s','/input/tests','-v'],check=True)
print('Protocol, RV32 runtime, settings and offline image/flash-tool tests passed.')

# Exercise production/diagnostic startup in the actual RV32 executable.
for mode in ['0','1']:
 with tempfile.TemporaryDirectory() as directory:
  result=subprocess.run(['qemu-riscv32','-L','/build/firmware-root','/input/local/artifacts/aa-phone-v821','127.0.0.1','1','missing-cert','missing-key','missing-video'],cwd=directory,env={**os.environ,'AA_DIAGNOSTIC':mode},capture_output=True,text=True)
  assert result.returncode==1 and 'Video file' in result.stderr,result.stderr
  for name in ['usb-rx.bin','aa-rx.bin']:
   assert (Path(directory)/name).exists()==(mode=='1'),(mode,name)
print('RV32 production mode creates no capture files; diagnostic opt-in verified.')
