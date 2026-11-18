"""USB startup must not wait for the system D-Bus socket used by Bluetooth."""
import gzip,os,signal,socket,subprocess,tempfile,time,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
class BootIndependence(unittest.TestCase):
 def test_usb_starts_without_dbus(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);lib=p/'lib';lib.mkdir();work=p/'work';log=p/'boot.log'
   for name in ['clock-service.sh','vehicle-log.sh','log-maintenance.sh','web-service.sh','radio-service.sh']:
    (lib/name).write_text('#!/bin/sh\nexit 0\n')
   for name in ['catplay-aa-rx','aa-phone-v821','aa_bulk.ko']:(lib/(name+'.gz')).write_bytes(gzip.compress(b'test'))
   for name in ['test-cert.pem','test-key.pem','mire.h264','wifi-env.sh']:(lib/name).write_text('test')
   (lib/'usb-test.sh').write_text('#!/bin/sh\necho USB_STARTED\n')
   script=(ROOT/'deploy/boot.sh').read_text().replace('/usr/lib/aa-bridge',str(lib)).replace('/var/log/aa-bridge-boot.log',str(log)).replace('/tmp/aa-autoboot',str(work)).replace('/sys/class/iphone/*',str(p/'absent-iphone/*'))
   result=subprocess.run(['sh','-c',script],capture_output=True,text=True,timeout=5)
   self.assertEqual(result.returncode,0,result.stderr)
   self.assertIn('USB_STARTED',result.stdout)
   self.assertIn('independent of D-Bus',result.stdout)
 def test_radio_waits_for_its_own_dbus(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);bus=p/'bus';(p/'state').mkdir()
   for name in ['certificate.p7b','identity.pk8']:(p/name).write_text('test')
   (p/'wifi-env.sh').write_text('SSID=test\n')
   (p/'catplay-aa-rx').write_text('#!/bin/sh\necho started > received\nexec sleep 30\n');(p/'catplay-aa-rx').chmod(0o700)
   script=(ROOT/'deploy/radio-service.sh').read_text().replace('/persist/',str(p)+'/')
   # Isolate daemon startup; this test verifies dependency gating, not BlueZ itself.
   script=script.replace('if ! pidof bluetoothd >/dev/null 2>&1 && [ "$attempts" -lt 3 ];then','if false;then')
   child=subprocess.Popen(['sh','-c',script],cwd=p,env={**os.environ,'AA_RADIO_WORK':str(p),'AA_DBUS_SOCKET':str(bus)},start_new_session=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   try:
    time.sleep(.2);self.assertFalse((p/'received').exists())
    with socket.socket(socket.AF_UNIX) as server:
     server.bind(str(bus));end=time.monotonic()+3
     while time.monotonic()<end and not (p/'received').exists():time.sleep(.05)
     self.assertTrue((p/'received').exists())
   finally:os.killpg(child.pid,signal.SIGTERM);child.wait()
