import subprocess,tempfile,os,time,http.client,json,shlex
folder=tempfile.mkdtemp(prefix='aa-web-test-')
p=subprocess.Popen(shlex.split(os.environ.get('AA_WEB_TEST_COMMAND','/build/aa/web-server-test')),env={**os.environ,'AA_SETTINGS_DIR':folder},stderr=subprocess.DEVNULL)
def req(method,path,body=None,headers={}):
 c=http.client.HTTPConnection('127.0.0.1',80,timeout=3);c.request(method,path,body,headers);r=c.getresponse();data=r.read();status=r.status;c.close();return status,data
try:
 for _ in range(30):
  try:assert req('GET','/name')==(200,b'Car');break
  except OSError:time.sleep(.05)
 else:raise AssertionError('HTTP server not ready')
 assert req('PUT','/name','Peugeot')[0]==200
 assert req('GET','/name')==(200,b'Peugeot')
 logo=open('/input/bridge-aa/carplay-rx/assets/car.png','rb').read()
 assert req('PUT','/logo.png',logo)[0]==200
 assert req('GET','/logo.png')==(200,logo)
 # Regression: RV32 metadata ABI crashed when the directory already existed.
 for _ in range(3):
  assert req('PUT','/name','Peugeot')[0]==200
  assert req('PUT','/logo.png',logo)[0]==200
  assert req('GET','/logo.png')==(200,logo)
 assert req('PUT','/logo.png',b'not PNG')[0]==400
 assert req('PUT','/name','bad\nname')[0]==400
 assert req('PUT','/name','Fiat',{'Origin':'http://other.invalid'})[0]==403
 assert req('GET','/name')==(200,b'Peugeot')
 assert req('GET','/')[0]==200
 defaults={'sidebar':'left','return_button':True,'touch':True,'fps':30,'theme':'auto','assist_buttons':True,'max_resolution':0,'scale_percent':100,'inset_top':'auto','inset_bottom':'auto','h264_poc2':True}
 assert json.loads(req('GET','/options')[1])==defaults
 assert req('PUT','/options','sidebar=right\nreturn_button=false\ntouch=false\nfps=15\ntheme=night\nassist_buttons=false\nmax_resolution=480\nscale_percent=125\ninset_top=0\ninset_bottom=40\nh264_poc2=0')[0]==200
 chosen={'sidebar':'right','return_button':False,'touch':True,'fps':15,'theme':'night','assist_buttons':False,'max_resolution':480,'scale_percent':125,'inset_top':'0','inset_bottom':'40','h264_poc2':False}
 assert json.loads(req('GET','/options')[1])==chosen
 assert req('PUT','/options','fps=99')[0]==400
 assert req('PUT','/options','inset_bottom=900')[0]==400
 assert json.loads(req('GET','/options')[1])==chosen
 # A server restart must reload the stored options and logo.
 p.terminate();p.wait()
 p=subprocess.Popen(shlex.split(os.environ.get('AA_WEB_TEST_COMMAND','/build/aa/web-server-test')),env={**os.environ,'AA_SETTINGS_DIR':folder},stderr=subprocess.DEVNULL)
 for _ in range(50):
  try:
   assert json.loads(req('GET','/options')[1])==chosen
   break
  except OSError:time.sleep(.05)
 else:raise AssertionError('restarted server not ready')
 assert req('GET','/logo.png')==(200,logo)
 assert req('PUT','/options','fps=10\nassist_buttons=false')[0]==200
 assert json.loads(req('GET','/options')[1])['fps']==10
 assert req('POST','/reset')[0]==200
 assert req('GET','/name')==(200,b'Car')
 assert json.loads(req('GET','/options')[1])==defaults
 print('HTTP settings: GET, persistence, upload, validation, origin guard and reset passed')
finally:p.terminate();p.wait()
