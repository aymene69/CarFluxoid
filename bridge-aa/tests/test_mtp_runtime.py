"""Exercise the real RV32 responder over a raw PTY with fragmented commands."""
import os,pty,select,shlex,struct,subprocess,tempfile,termios,time,tty,unittest
from pathlib import Path
import socket
class MtpRuntime(unittest.TestCase):
 def test_discovery_session_and_disconnect(self):
  master,slave=pty.openpty();tty.setraw(slave);path=os.ttyname(slave)
  command=shlex.split(os.environ['AA_MTP_TEST_COMMAND'])+['--mtp',path]
  child=subprocess.Popen(command,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
  def read_exact(count):
   out=b'';deadline=time.monotonic()+5
   while len(out)<count:
    if time.monotonic()>deadline:raise AssertionError('MTP response timed out')
    if select.select([master],[],[],.1)[0]:out+=os.read(master,count-len(out))
   return out
  def packet():
   h=read_exact(12);size,kind,code,tx=struct.unpack('<IHHI',h)
   self.assertGreaterEqual(size,12);self.assertLessEqual(size,512)
   return kind,code,tx,read_exact(size-12)
  def request(op,tx,*params):
   data=struct.pack('<IHHI',12+len(params)*4,1,op,tx)+b''.join(struct.pack('<I',v) for v in params)
   os.write(master,data[:3]);time.sleep(.01);os.write(master,data[3:])
  try:
   request(0x1001,0x12345678);kind,code,tx,data=packet()
   self.assertEqual((kind,code,tx),(2,0x1001,0x12345678));self.assertEqual(struct.unpack_from('<H',data)[0],100)
   self.assertIn('CarFluxoid'.encode('utf-16le'),data)
   self.assertEqual(packet(),(3,0x2001,0x12345678,b''))
   request(0x1002,1,7);self.assertEqual(packet(),(3,0x2001,1,b''))
   request(0x1004,2);self.assertEqual(packet(),(2,0x1004,2,b'\0'*4));self.assertEqual(packet(),(3,0x2001,2,b''))
   request(0x9999,0xdeadbeef);self.assertEqual(packet(),(3,0x2005,0xdeadbeef,b''))
   request(0x1003,3);self.assertEqual(packet(),(3,0x2001,3,b''))
   os.close(master);master=-1;child.wait(timeout=5)
   self.assertEqual(child.returncode,0)
   self.assertIn(b'MTP_CLOSED',child.stderr.read())
  finally:
   if child.poll() is None:child.terminate();child.wait(timeout=5)
   child.stderr.close();os.close(slave)
   if master>=0:os.close(master)
 def test_waits_for_configured_accessory_without_replacing_video_listener(self):
  with tempfile.TemporaryDirectory() as td:
   p=Path(td);master,slave=pty.openpty();tty.setraw(slave)
   (p/'accessory_active').write_text('N');(p/'usb_online').write_text('N')
   (p/'mire.h264').write_bytes(b'test')
   log=p/'phone.log';sock=p/'video.sock'
   env={**os.environ,'AA_MTP_DISCOVERY':'1','AA_AUTONOMOUS':'1','AA_VIDEO_SOCKET':str(sock),'AA_TEST_USB_STATE_DIR':str(p)}
   command=shlex.split(os.environ['AA_MTP_TEST_COMMAND'])+['--raw',os.ttyname(slave),'missing-cert','missing-key',str(p/'mire.h264')]
   with log.open('wb') as output:
    child=subprocess.Popen(command,stdout=subprocess.DEVNULL,stderr=output,env=env)
    peer=socket.socket(socket.AF_UNIX)
    try:
     deadline=time.monotonic()+5
     while time.monotonic()<deadline and not sock.exists():time.sleep(.02)
     self.assertTrue(sock.exists());peer.connect(str(sock));inode=sock.stat().st_ino
     (p/'accessory_active').write_text('Y');time.sleep(.2)
     self.assertNotIn('USB_CONTINUOUS_READER',log.read_text())
     (p/'usb_online').write_text('Y');deadline=time.monotonic()+5
     while time.monotonic()<deadline and 'USB_CONTINUOUS_READER' not in log.read_text():time.sleep(.02)
     self.assertIn('MTP_SWITCHED_TO_ACCESSORY',log.read_text())
     self.assertIn('USB_CONTINUOUS_READER',log.read_text())
     self.assertEqual(sock.stat().st_ino,inode);self.assertIsNone(child.poll())
     # Deliver a version request; TLS is intentionally not configured in this test.
     os.write(master,bytes.fromhex('00030006000100010007'))
     child.wait(timeout=5);self.assertIn('AA VERSION 1.7 negotiated',log.read_text())
    finally:
     peer.close();os.close(master);os.close(slave)
     if child.poll() is None:child.kill();child.wait()
if __name__=='__main__':unittest.main()
