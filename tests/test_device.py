"""Offline tests: preservation, rejected hardware/images and simulated SPI readback."""
import sys,unittest,tempfile,gzip,struct,json,hashlib
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'tooling'));import device

def fixture():
 def node(n):
  b=struct.pack('>I',1)+n.encode()+b'\0';return b+b'\0'*((-len(b))%4)
 raw=b'kernel fixture';device.KERNELS.add(device.sha(raw));data=gzip.compress(raw,mtime=0)
 s=node('')+node('images')+node('kernel-1')+struct.pack('>III',3,len(data),0)+data+b'\0'*((-len(data))%4)+struct.pack('>IIII',2,2,2,9)
 total=56+len(s)+5;fit=struct.pack('>10I',0xd00dfeed,total,56,56+len(s),40,17,16,0,5,len(s))+b'\0'*16+s+b'data\0'
 image=bytearray(b'B'*device.SIZE);image[0x40000:device.START]=fit+b'\xff'*(device.START-0x40000-len(fit));image[device.END:]=b'P'*(device.SIZE-device.END);return bytes(image)
class MemoryFel(device.Fel):
 def __init__(self,data,tamper=False):self.memory=data;self.writes=0;self.tamper=tamper
 def identify(self):pass
 def run(self,*args,timeout=1800):
  assert args[0]=='spinor'
  if args[1]=='read':
   data=self.memory
   if self.tamper and self.writes:data=b'X'+data[1:]
   Path(args[4]).write_bytes(data)
  else:
   self.writes+=1;start=int(args[2],16);data=Path(args[3]).read_bytes();self.memory=self.memory[:start]+data+self.memory[start+len(data):]
  return ''
class DeviceTests(unittest.TestCase):
 def setUp(self):self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.before=fixture()
 def tearDown(self):self.tmp.cleanup()
 def compose(self):
  backup=self.root/'original.bin';backup.write_bytes(self.before);art=self.root/'artifacts';art.mkdir();root=bytearray(b'R'*(device.END-device.START));root[1036]=12;(art/'root.partition.bin').write_bytes(root);(art/'manifest.json').write_text(json.dumps({'root_sha256':device.sha(root)}));out=self.root/'image.c2aflash';device.compose(backup,art,out);return out
 def test_compose_preserves_identity_boot_and_kernel(self):
  out=self.compose();after=out.read_bytes();self.assertEqual(after[:device.START],self.before[:device.START]);self.assertEqual(after[device.END:],self.before[device.END:]);self.assertNotEqual(after[device.START:device.END],self.before[device.START:device.END])
 def test_invalid_kernel_and_geometry_rejected(self):
  with self.assertRaises(ValueError):device.inspect(b'bad')
  raw=bytearray(self.before);raw[0x40000]=0
  with self.assertRaises(ValueError):device.inspect(raw)
 def test_spi_update_and_readback(self):
  out=self.compose();fel=MemoryFel(self.before);fel.flash(out,self.root/'flash');self.assertEqual(fel.memory,out.read_bytes());self.assertEqual(fel.writes,1)
 def test_readback_failure_is_reported(self):
  out=self.compose();fel=MemoryFel(self.before,True)
  with self.assertRaisesRegex(RuntimeError,'Readback mismatch'):fel.flash(out,self.root/'failed')
 def test_kernel_change_prevents_write(self):
  out=self.compose();target=bytearray(out.read_bytes());target[0]=0;out.write_bytes(target);fel=MemoryFel(self.before)
  with self.assertRaises(ValueError):fel.flash(out,self.root/'rejected')
  self.assertEqual(fel.writes,0)
if __name__=='__main__':unittest.main()
